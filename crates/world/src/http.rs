use axum::{
    extract::{ws::{WebSocket, WebSocketUpgrade, Message}, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, patch, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::sync::Arc;
use sqlx::SqlitePool;
use tokio::sync::{mpsc, oneshot};
use crate::agent_supervisor::AgentSupervisor;
use crate::loop_::{Cmd, Handle};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub handle: Handle,
    pub catalog: Arc<tokio::sync::RwLock<std::collections::HashMap<String, serde_json::Value>>>,
    pub supervisor: Arc<AgentSupervisor>,
    pub max_review_rounds: u32,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({"ok": true})) }))
        .route("/metrics", get(metrics_handler))
        .route("/api/backend-catalog", get(api_catalog))
        .route("/api/backend-catalog/recheck", post(api_recheck))
        .route("/api/startups", post(crate::api_startups::create_startup))
        .route("/api/admin/tasks", post(crate::api_tasks::create_task))
        .route(
            "/api/startups/:id",
            patch(patch_startup).delete(crate::api_startups::delete_startup),
        )
        .route(
            "/api/agents/:agent_id/skills",
            get(crate::api_skills::get_agent_skills),
        )
        .route("/ws/console", get(ws_console))
        .route("/ws/worker", get(ws_worker))
        // M9.10 A1' — MCP-over-HTTP at the world. See `mcp_http.rs` for the
        // Bearer auth + JSON-RPC envelope + dispatch routing.
        .route("/mcp", post(crate::mcp_http::handle_request))
        .with_state(Arc::new(state))
}

/// Operator endpoint for raising/lowering a startup's budget cap. Auto-resume
/// after a 100% pause is implicit: `budget::newly_crossed` only trips on
/// transitions, so raising the cap above current spend prevents subsequent
/// `report_budget` reports from re-tripping the 100% threshold.
async fn patch_startup(
    Path(id): Path<String>,
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    // Operator token may arrive as `Authorization: Bearer <tok>` or as the
    // bare `X-Operator-Token: <tok>` header — accept either to match the
    // console hello path's tolerance.
    let tok = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or(Some(s)))
        .or_else(|| headers.get("x-operator-token").and_then(|v| v.to_str().ok()))
        .unwrap_or("");
    if crate::auth::validate_operator_token(&s.pool, tok).await.is_err() {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error":"unauthorized"}))).into_response();
    }
    let new_cap = match body.get("budget_cap_usd").and_then(|v| v.as_f64()) {
        Some(v) if v.is_finite() && v >= 0.0 && v < 1_000_000.0 => v,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"missing or invalid budget_cap_usd"})),
            )
                .into_response();
        }
    };
    let r = sqlx::query("UPDATE startups SET budget_cap_usd = ? WHERE id = ?")
        .bind(new_cap)
        .bind(&id)
        .execute(&s.pool)
        .await;
    match r {
        Ok(res) if res.rows_affected() == 0 => {
            (StatusCode::NOT_FOUND, Json(json!({"error":"startup not found"}))).into_response()
        }
        Ok(_) => Json(json!({"ok": true, "id": id, "budget_cap_usd": new_cap})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn api_catalog(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let m = s.catalog.read().await;
    Json(serde_json::to_value(&*m).unwrap())
}

async fn api_recheck(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let new_cat = crate::backend_catalog::probe_all().await;
    let new_json: std::collections::HashMap<_, _> = new_cat
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap()))
        .collect();
    *s.catalog.write().await = new_json.clone();
    let _ = s.handle.tx.send(Cmd::BackendCatalogUpdated(new_json.clone())).await;
    Json(serde_json::json!({"ok": true, "entries": new_json}))
}

async fn ws_console(ws: WebSocketUpgrade, State(s): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |sock| handle_console(sock, s))
}
async fn ws_worker(ws: WebSocketUpgrade, State(s): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |sock| handle_worker(sock, s))
}

/// Build the `world_view_snapshot` frame the console expects on connect and
/// after every world-view change. The frame embeds the raw `WorldView`
/// fields (tick_seq, backend_catalog, avatars) plus two SQL-sourced lists
/// (`startups`, `tasks`) that the frontend reducer in
/// `packages/frontend/src/store.ts` reads directly into its sidebar +
/// kanban views.
///
/// Note: each invocation runs two SELECT statements. Phase-0 worlds are
/// tiny (a few startups, dozens of tasks) so the cost is negligible, but
/// this is a TODO for caching once tick rates climb past a few Hz —
/// e.g. memoize on `tick_seq` + a cheap "tasks dirty" counter.
/// P3 Theme D: Prometheus text exposition endpoint at `/metrics`.
/// Renders process-wide counters + per-scrape gauges from SQL + view.
async fn metrics_handler(State(s): State<Arc<AppState>>) -> Response {
    let view = s.handle.view_rx.borrow().clone();
    let body = crate::metrics::render(&s.pool, &view).await;
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
        .into_response()
}

pub async fn build_console_snapshot(
    pool: &SqlitePool,
    view: &crate::state::WorldView,
    max_review_rounds: u32,
) -> serde_json::Value {
    // Active startups, plus the most recent system_event ts so the sidebar
    // can flag stale runs at a glance. Falls back to `created_at` when no
    // event exists yet. Theme G slice 2: include auto_steal flag +
    // threshold so the admin-only settings popover hydrates from the
    // snapshot rather than a side fetch.
    let startups: Vec<serde_json::Value> = sqlx::query_as::<
        _,
        (String, String, f64, f64, i64, i64, i64),
    >(
        "SELECT id, name, budget_spent_usd, budget_cap_usd, \
         COALESCE((SELECT MAX(ts) FROM system_events WHERE startup_id = startups.id), created_at), \
         auto_steal_enabled, auto_steal_after_secs \
         FROM startups WHERE status = 'active'",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, name, spent, cap, last_ts, autosteal, autosteal_secs)| {
        json!({
            "id": id,
            "name": name,
            "budget_spent_usd": spent,
            "budget_cap_usd": cap,
            "last_event_ts": last_ts,
            "auto_steal_enabled": autosteal != 0,
            "auto_steal_after_secs": autosteal_secs,
        })
    })
    .collect();

    // In-flight + pending tasks (everything except `done` / `failed`) so
    // the kanban shows the live work surface without flooding on history.
    // `artifact_path` is set by `handle_task_done` (mcp_dispatch.rs) when the
    // engineer submits work. Status flips in_progress → awaiting_review in
    // the same UPDATE, so the operator console sees the canonical path
    // (`workspaces/<sid>/artifacts/<tid>.md`) on the kanban card while the
    // manager reviews. Spec § 11.4 — the operator-visible proof of "artifact
    // landed at the canonical path."
    // Theme G slice 3: include `blocked_on` + `deadline_at` so Kanban
    // cards can render the blocked / deadline badges from E2 without a
    // side fetch.
    let tasks: Vec<serde_json::Value> = sqlx::query_as::<
        _,
        (String, String, String, String, Option<String>, Option<String>, i64, Option<String>, Option<String>, Option<i64>),
    >(
        "SELECT id, startup_id, title, status, assignee_agent_id, required_room, review_round, artifact_path, blocked_on, deadline_at \
         FROM tasks WHERE status NOT IN ('done', 'failed')",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, startup_id, title, status, assignee, required_room, review_round, artifact_path, blocked_on, deadline_at)| {
        json!({
            "id": id,
            "startup_id": startup_id,
            "title": title,
            "status": status,
            "assignee_agent_id": assignee,
            "required_room": required_room,
            "review_round": review_round,
            "max_review_rounds": max_review_rounds,
            "artifact_path": artifact_path,
            "blocked_on": blocked_on,
            "deadline_at": deadline_at,
        })
    })
    .collect();

    // Theme G slice 2: enrich each avatar with `is_peer_reviewer` so the
    // admin-only AgentsPanel can render the per-agent toggle without a
    // side fetch. We don't store the flag on AvatarView (would force a
    // 46-test-file edit cascade for the literal construction sites);
    // instead we join from `agents` here and merge into the serialized
    // avatar objects. Missing rows → false (the SQL default).
    let peer_reviewer_rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT id, is_peer_reviewer FROM agents")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    let peer_reviewer: std::collections::HashMap<String, bool> = peer_reviewer_rows
        .into_iter()
        .map(|(id, flag)| (id, flag != 0))
        .collect();
    let avatars: Vec<serde_json::Value> = view
        .avatars
        .values()
        .map(|a| {
            let mut v = serde_json::to_value(a).unwrap_or(serde_json::Value::Null);
            if let Some(obj) = v.as_object_mut() {
                let pr = peer_reviewer.get(&a.agent_id).copied().unwrap_or(false);
                obj.insert("is_peer_reviewer".into(), json!(pr));
            }
            v
        })
        .collect();

    json!({
        "type": "world_view_snapshot",
        "v": 1,
        "snapshot": {
            "tick_seq": view.tick_seq,
            "backend_catalog": view.backend_catalog,
            "avatars": avatars,
            "startups": startups,
            "tasks": tasks,
        },
    })
}

async fn handle_console(mut socket: WebSocket, state: Arc<AppState>) {
    // Phase 1: hello + auth on the unsplit socket so we can early-return
    // before splitting (no split needs to be undone on auth failure).
    let Some(Ok(Message::Text(first))) = socket.recv().await else { return; };
    let parsed: serde_json::Value = match serde_json::from_str(&first) { Ok(v) => v, Err(_) => return };
    if parsed.get("type") != Some(&serde_json::Value::String("hello".into())) { return; }
    let token = parsed.get("operator_token").and_then(|v| v.as_str()).unwrap_or("");
    let identity = match crate::auth::validate_operator_token(&state.pool, token).await {
        Ok(id) => id,
        Err(_) => {
            let _ = socket.send(Message::Text(r#"{"type":"auth_error"}"#.into())).await;
            return;
        }
    };

    // P3 carry-forward: tell the frontend who's connected so it can gate
    // admin-only UI surfaces (OperatorsPanel, the SkillsPanel global toggle).
    // Token is NOT echoed; identity-only. Failure to send drops the
    // connection — the snapshot send below will fail identically.
    let hello_ok = json!({
        "type": "hello_ok",
        "v": 1,
        "operator_id": identity.id,
        "operator_name": identity.name,
        "role": identity.role.as_str(),
    });
    if socket.send(Message::Text(hello_ok.to_string().into())).await.is_err() {
        return;
    }

    // Subscribe to the world view watcher BEFORE sending the initial snapshot
    // so we don't miss a tick that fires between the borrow + the subsequent
    // `changed()` await.
    let mut view_rx = state.handle.view_rx.clone();
    // Mark the current value as "seen" so the first `changed()` below only
    // fires on a fresh write (otherwise it returns immediately and we'd push
    // a duplicate of the initial snapshot).
    view_rx.borrow_and_update();

    // Subscribe to the broadcast channel for live Chat/Directive/SystemEvent
    // frames. Subscribed BEFORE the initial snapshot so we don't miss any
    // events that fire in between. Lagged is treated as fatal-close (see
    // the third select! arm below).
    let mut event_rx = state.handle.event_tx.subscribe();

    // Send the initial snapshot. Phase 0 worlds are small enough that we
    // skip the `chunk_snapshot` transport (M1.11) — TODO M11+: route through
    // chunk_snapshot when the serialized payload exceeds the 256 KiB threshold
    // already enforced for worker view fans.
    {
        let view = state.handle.view_rx.borrow().clone();
        let frame = build_console_snapshot(&state.pool, &view, state.max_review_rounds).await;
        if socket.send(Message::Text(frame.to_string().into())).await.is_err() {
            return;
        }
    }

    // P2.2: send the SkillsSnapshot right after WorldViewSnapshot so the
    // SkillsPanel hydrates immediately. Failure here logs and proceeds —
    // skills are optional product surface.
    {
        let by_startup = crate::skills::list_all_with_attachments(&state.pool)
            .await
            .unwrap_or_default();
        let payload: serde_json::Value = by_startup
            .into_iter()
            .map(|(sid, rows)| {
                let arr: Vec<serde_json::Value> = rows
                    .iter()
                    .map(crate::skills::skill_with_attachments_to_json)
                    .collect();
                (sid, serde_json::Value::Array(arr))
            })
            .collect::<serde_json::Map<_, _>>()
            .into();
        let frame = json!({
            "type": "skills_snapshot",
            "v": 1,
            "startups": payload,
        });
        if socket.send(Message::Text(frame.to_string().into())).await.is_err() {
            return;
        }
    }

    // P5 Theme A: register presence on connect, broadcast initial list.
    {
        let now = chrono::Utc::now().timestamp();
        let _ = crate::presence::upsert(
            &state.handle.presence,
            &identity.id,
            &identity.name,
            identity.role.as_str(),
            None,
            now,
        ).await;
        let snap = crate::presence::snapshot(&state.handle.presence).await;
        let _ = state.handle.event_tx.send(
            crate::protocol::ConsoleOutbound::OperatorPresence {
                v: 1,
                presences: serde_json::to_value(&snap).unwrap_or(serde_json::Value::Null),
            },
        );
    }

    // Phase 2: split + select! loop. Inbound frames go to the world via
    // `Cmd::HandleConsoleMsg`; world-view changes are pushed back as fresh
    // snapshots. Mirrors the structure used by `handle_worker`.
    let (mut sender, mut receiver) = socket.split();
    loop {
        tokio::select! {
            inbound = receiver.next() => {
                match inbound {
                    Some(Ok(Message::Text(txt))) => {
                        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&txt) else { continue; };
                        // P5 Theme A: short-circuit presence heartbeats so they
                        // don't traverse the world loop (presence is not world
                        // state). Heartbeats with a changed focus broadcast a
                        // fresh presence list; same-focus pings just refresh
                        // last_seen_at.
                        if msg.get("type").and_then(|v| v.as_str()) == Some("presence_heartbeat") {
                            let focused = msg.get("focused_startup_id")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let now = chrono::Utc::now().timestamp();
                            let changed = crate::presence::upsert(
                                &state.handle.presence,
                                &identity.id,
                                &identity.name,
                                identity.role.as_str(),
                                focused,
                                now,
                            ).await;
                            if changed {
                                let snap = crate::presence::snapshot(&state.handle.presence).await;
                                let _ = state.handle.event_tx.send(
                                    crate::protocol::ConsoleOutbound::OperatorPresence {
                                        v: 1,
                                        presences: serde_json::to_value(&snap)
                                            .unwrap_or(serde_json::Value::Null),
                                    },
                                );
                            }
                            continue;
                        }
                        let (tx, rx) = oneshot::channel();
                        let _ = state.handle.tx.send(Cmd::HandleConsoleMsg {
                            msg,
                            identity: identity.clone(),
                            reply: tx,
                        }).await;
                        if let Ok(reply) = rx.await {
                            if sender.send(Message::Text(reply.to_string().into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(_)) => { /* ignore non-text frames */ }
                    _ => break,
                }
            }
            changed = view_rx.changed() => {
                if changed.is_err() { break; }
                let view = view_rx.borrow_and_update().clone();
                let frame = build_console_snapshot(&state.pool, &view, state.max_review_rounds).await;
                if sender.send(Message::Text(frame.to_string().into())).await.is_err() {
                    break;
                }
            }
            event = event_rx.recv() => {
                match event {
                    Ok(frame) => {
                        let json = match serde_json::to_string(&frame) {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::warn!(component = "handle_console", err = %e,
                                    "failed to serialize broadcast frame");
                                continue;
                            }
                        };
                        if sender.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(component = "handle_console", lagged = n,
                            "console subscriber lagged; closing WS to force resync");
                        break;  // frontend will reconnect to a fresh snapshot
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    // P5 Theme A: drop presence on disconnect + broadcast the new list
    // so peers see us go offline immediately. GC also drops stale
    // entries every 30s but explicit drop is faster.
    if crate::presence::drop_entry(&state.handle.presence, &identity.id).await {
        let snap = crate::presence::snapshot(&state.handle.presence).await;
        let _ = state.handle.event_tx.send(
            crate::protocol::ConsoleOutbound::OperatorPresence {
                v: 1,
                presences: serde_json::to_value(&snap).unwrap_or(serde_json::Value::Null),
            },
        );
    }
}

async fn handle_worker(mut socket: WebSocket, state: Arc<AppState>) {
    // Phase 1: hello + auth (still on the unsplit socket).
    let Some(Ok(Message::Text(first))) = socket.recv().await else { return; };
    let parsed: serde_json::Value = match serde_json::from_str(&first) { Ok(v) => v, Err(_) => return };
    if parsed.get("type") != Some(&serde_json::Value::String("hello".into())) { return; }
    let agent_id = parsed.get("agent_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let secret = parsed.get("secret").and_then(|v| v.as_str()).unwrap_or("");
    if crate::auth::validate_agent_secret(&state.pool, &agent_id, secret).await.is_err() {
        let _ = socket.send(Message::Text(r#"{"type":"auth_error"}"#.into())).await;
        return;
    }

    // Ack the hello so the worker client (`packages/worker/src/ws.ts::connect`)
    // can resolve its `await` instead of timing out. Contract: `{type:"ok",
    // kind:"hello"}` — see `crates/world/tests/ws_auth.rs` and
    // `packages/worker/test/ws_hello.test.ts`.
    let ack = serde_json::json!({"type":"ok","kind":"hello"});
    let _ = socket.send(Message::Text(ack.to_string().into())).await;

    // Phase 2: split + register out_bus, then drive a select!() loop that
    // races inbound WS frames against outbound messages pushed by the world
    // loop (e.g. move_complete / move_failed).
    let (mut sender, mut receiver) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<serde_json::Value>(64);
    let _ = state.handle.tx.send(Cmd::RegisterWorker {
        agent_id: agent_id.clone(),
        tx: out_tx,
    }).await;

    loop {
        tokio::select! {
            inbound = receiver.next() => {
                match inbound {
                    Some(Ok(Message::Text(txt))) => {
                        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&txt) else { continue; };
                        let (tx, rx) = oneshot::channel();
                        let _ = state.handle.tx.send(Cmd::HandleWorkerMsg {
                            agent_id: agent_id.clone(), msg, reply: tx
                        }).await;
                        if let Ok(reply) = rx.await {
                            if sender.send(Message::Text(reply.to_string().into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(_)) => { /* ignore non-text frames */ }
                    _ => break,
                }
            }
            Some(out_msg) = out_rx.recv() => {
                if sender.send(Message::Text(out_msg.to_string().into())).await.is_err() {
                    break;
                }
            }
            else => break,
        }
    }

    let _ = state.handle.tx.send(Cmd::UnregisterWorker { agent_id }).await;
}
