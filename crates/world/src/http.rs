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
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({"ok": true})) }))
        .route("/api/backend-catalog", get(api_catalog))
        .route("/api/backend-catalog/recheck", post(api_recheck))
        .route("/api/startups", post(crate::api_startups::create_startup))
        .route(
            "/api/startups/:id",
            patch(patch_startup).delete(crate::api_startups::delete_startup),
        )
        .route("/ws/console", get(ws_console))
        .route("/ws/worker", get(ws_worker))
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
async fn build_console_snapshot(
    pool: &SqlitePool,
    view: &crate::state::WorldView,
) -> serde_json::Value {
    // Active startups, plus the most recent system_event ts so the sidebar
    // can flag stale runs at a glance. Falls back to `created_at` when no
    // event exists yet.
    let startups: Vec<serde_json::Value> = sqlx::query_as::<
        _,
        (String, String, f64, f64, i64),
    >(
        "SELECT id, name, budget_spent_usd, budget_cap_usd, \
         COALESCE((SELECT MAX(ts) FROM system_events WHERE startup_id = startups.id), created_at) \
         FROM startups WHERE status = 'active'",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, name, spent, cap, last_ts)| {
        json!({
            "id": id,
            "name": name,
            "budget_spent_usd": spent,
            "budget_cap_usd": cap,
            "last_event_ts": last_ts,
        })
    })
    .collect();

    // In-flight + pending tasks (everything except `done` / `failed`) so
    // the kanban shows the live work surface without flooding on history.
    let tasks: Vec<serde_json::Value> = sqlx::query_as::<
        _,
        (String, String, String, String, Option<String>, Option<String>),
    >(
        "SELECT id, startup_id, title, status, assignee_agent_id, required_room \
         FROM tasks WHERE status NOT IN ('done', 'failed')",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, startup_id, title, status, assignee, required_room)| {
        json!({
            "id": id,
            "startup_id": startup_id,
            "title": title,
            "status": status,
            "assignee_agent_id": assignee,
            "required_room": required_room,
        })
    })
    .collect();

    json!({
        "type": "world_view_snapshot",
        "v": 1,
        "snapshot": {
            "tick_seq": view.tick_seq,
            "backend_catalog": view.backend_catalog,
            "avatars": view.avatars.values().collect::<Vec<_>>(),
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
    if crate::auth::validate_operator_token(&state.pool, token).await.is_err() {
        let _ = socket.send(Message::Text(r#"{"type":"auth_error"}"#.into())).await;
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

    // Send the initial snapshot. Phase 0 worlds are small enough that we
    // skip the `chunk_snapshot` transport (M1.11) — TODO M11+: route through
    // chunk_snapshot when the serialized payload exceeds the 256 KiB threshold
    // already enforced for worker view fans.
    {
        let view = state.handle.view_rx.borrow().clone();
        let frame = build_console_snapshot(&state.pool, &view).await;
        if socket.send(Message::Text(frame.to_string().into())).await.is_err() {
            return;
        }
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
                        let (tx, rx) = oneshot::channel();
                        let _ = state.handle.tx.send(Cmd::HandleConsoleMsg { msg, reply: tx }).await;
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
                let frame = build_console_snapshot(&state.pool, &view).await;
                if sender.send(Message::Text(frame.to_string().into())).await.is_err() {
                    break;
                }
            }
        }
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
