//! MCP tool dispatch on the world server. Each handler validates permissions,
//! performs the SQL mutation, fans relevant events, and returns either an
//! `mcp_reply` or `mcp_error` envelope.
//!
//! Permission model (Phase 0):
//! - Same-startup gate is universal: an agent may only mutate state inside its
//!   own `startup_id`. The cross-startup property test in `tests/mcp_handlers`
//!   pins this invariant.
//! - "Manager" is structural rather than role-named: a caller is the manager
//!   of a task when (a) the task has a parent and the caller is the parent
//!   task's assignee, or (b) the task has no parent and the caller is the
//!   recorded `agents.manager_id` of the assignee. Phase 0 keeps this query
//!   intentionally cheap; Phase 1 will fold the org graph into a richer
//!   permission predicate alongside `permissions::can_send_directive`.
//! - `verify` runs `read_assert` and `lint_json` in-process; `lint_markdown`
//!   and `lint_typescript` return a deferred-stub envelope (M3+ owns the TS
//!   sidecar wiring).
//! - `ask_peer` returns `{response: null}` immediately. Phase 0 has no peers
//!   replying to peers; M3+ wires the round-trip semantics.

use crate::move_sys::{self, PathStore, StartMoveResult};
use crate::path::RoomGraph;
use crate::persist;
use crate::sandbox;
use crate::seed::TownLayout;
use crate::state::{AvatarView, WorldView};
use crate::task_sm::{next, Actor, TaskStatus, Transition};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Phase 0 hard cap on review rounds before the world auto-escalates.
/// M9 hardening should move this to `cliptown.toml [supervisor] max_review_rounds`.
pub(crate) const MAX_REVIEW_ROUNDS: u32 = 3;

/// Hard cap (in unicode chars) on user-supplied body strings on the chat /
/// directive / review-feedback paths. Every body that crosses this limit
/// would get cloned into the broadcast channel (capacity 4096, lag-loss
/// fatal-closes the WS), the SQL `messages` row, and the frontend's
/// 500-entry messages array — a worker or operator with unbounded body
/// could push real events out of the buffer and starve the operator
/// console. Guarded at the three producer call sites:
/// `cmd_console::OperatorDirective`, `mcp_dispatch::handle_speak`,
/// `mcp_dispatch::handle_task_request_changes`.
pub(crate) const MAX_BODY_LENGTH: usize = 4096;

/// Helper for the MCP-dispatch handlers: reject overlong bodies before any
/// side effect. Returns `Err(("body_too_long", message))` so the caller's
/// `HandlerResult` flow already maps to `mcp_error{code:"body_too_long"}`.
pub(crate) fn check_body_length(field: &str, body: &str) -> Result<(), (String, String)> {
    let len = body.chars().count();
    if len > MAX_BODY_LENGTH {
        return Err((
            "body_too_long".into(),
            format!(
                "{} exceeds {} chars (got {})",
                field, MAX_BODY_LENGTH, len
            ),
        ));
    }
    Ok(())
}

type HandlerResult = Result<Value, (String, String)>;

pub async fn dispatch(
    world: &mut WorldView,
    paths: &mut PathStore,
    layout: &TownLayout,
    graph: &RoomGraph,
    out_bus: &HashMap<String, mpsc::Sender<Value>>,
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    agent_id: &str,
    msg: Value,
) -> Value {
    let tool = msg
        .get("tool")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let args = msg.get("args").cloned().unwrap_or(Value::Null);
    let corr_id = msg
        .get("corr_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Snapshot the caller — we copy out so we don't hold an immutable borrow
    // on `world` while subsequent handlers mutate it (move_intent, speak).
    let caller = match world.avatars.get(agent_id).cloned() {
        Some(a) => a,
        None => return mcp_err(&corr_id, "unknown_agent", "agent not found in world"),
    };

    // P3 Theme D follow-up: structured tracing for every MCP dispatch.
    // We log an event-pair (enter + exit-with-elapsed) instead of holding a
    // Span across `.await` boundaries — the WS loop's task captures a `!Send`
    // Span guard, which breaks tokio::spawn's Send bound. The downside is
    // that nested handler logs aren't visually grouped under the dispatch in
    // a console subscriber, but structured backends correlate via `corr_id`.
    let dispatch_start = std::time::Instant::now();
    tracing::debug!(
        component = "mcp_dispatch",
        event = "enter",
        tool = %tool,
        agent_id = %caller.agent_id,
        startup_id = %caller.startup_id,
        corr_id = %corr_id,
    );

    // P3 Theme D: per-call counter for /metrics.
    crate::metrics::COUNTERS.inc_call();

    let result: HandlerResult = match tool.as_str() {
        "move_intent" => {
            handle_move_intent(world, paths, layout, graph, pool, event_tx, &caller, args).await
        }
        "speak" => handle_speak(world, out_bus, pool, event_tx, &caller, args).await,
        "task_done" => handle_task_done(world, out_bus, pool, &caller, args).await,
        "task_failed" => handle_task_failed(world, pool, &caller, args).await,
        "subtask_create" => handle_subtask_create(out_bus, pool, &caller, args).await,
        "task_accept" => handle_task_accept(out_bus, pool, &caller, args).await,
        "task_request_changes" => handle_task_request_changes(out_bus, pool, event_tx, &caller, args).await,
        "accept_proposal" => handle_accept_proposal(pool, &caller, args).await,
        "reject_proposal" => handle_reject_proposal(pool, &caller, args).await,
        "hypothesis_state" => handle_epistemic_append(pool, &caller, args, "hypothesis_state").await,
        "test_record" => handle_epistemic_append(pool, &caller, args, "test_record").await,
        "hypothesis_resolve" => {
            handle_epistemic_append(pool, &caller, args, "hypothesis_resolve").await
        }
        "verify" => handle_verify(&caller, args).await,
        "ask_peer" => handle_ask_peer(args).await,
        "observe_world" => handle_observe_world(world, pool, &caller, args).await,
        "read_artifact" => handle_read_artifact(&caller, args).await,
        "skill_upsert" => handle_skill_upsert(pool, event_tx, &caller, args).await,
        "skill_list" => handle_skill_list(pool, &caller, args).await,
        "skill_attach" => handle_skill_attach(pool, event_tx, &caller, args).await,
        "skill_detach" => handle_skill_detach(pool, event_tx, &caller, args).await,
        "skill_delete" => handle_skill_delete(pool, event_tx, &caller, args).await,
        "task_set_preference" => handle_task_set_preference(pool, event_tx, &caller, args).await,
        "skill_file_upsert" => handle_skill_file_upsert(pool, event_tx, &caller, args).await,
        "skill_file_delete" => handle_skill_file_delete(pool, event_tx, &caller, args).await,
        "skill_list_revisions" => handle_skill_list_revisions(pool, &caller, args).await,
        "skill_revert" => handle_skill_revert(pool, event_tx, &caller, args).await,
        "task_set_blocking" => handle_task_set_blocking(pool, event_tx, &caller, args).await,
        _ => Err((
            "unknown_tool".into(),
            format!("no handler for tool: {}", tool),
        )),
    };

    let elapsed_us = dispatch_start.elapsed().as_micros() as u64;
    match result {
        Ok(v) => {
            tracing::debug!(
                component = "mcp_dispatch",
                event = "exit",
                tool = %tool,
                corr_id = %corr_id,
                elapsed_us,
                outcome = "ok",
            );
            json!({"type":"mcp_reply","v":1,"corr_id":corr_id,"result":v})
        }
        Err((code, message)) => {
            crate::metrics::COUNTERS.inc_error();
            tracing::info!(
                component = "mcp_dispatch",
                event = "exit",
                tool = %tool,
                corr_id = %corr_id,
                elapsed_us,
                outcome = "error",
                code = %code,
            );
            mcp_err(&corr_id, &code, &message)
        }
    }
}

fn mcp_err(corr_id: &str, code: &str, message: &str) -> Value {
    json!({"type":"mcp_error","v":1,"corr_id":corr_id,"code":code,"message":message})
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Load (status, startup_id, parent_id, assignee, review_round, artifact_path)
/// for a task. Returns Err if the row is missing or the status string isn't one
/// the state machine recognizes.
async fn load_task(
    pool: &SqlitePool,
    task_id: &str,
) -> Result<TaskRow, (String, String)> {
    let row: Option<(String, String, Option<String>, Option<String>, i64, Option<String>)> =
        sqlx::query_as(
            "SELECT status, startup_id, parent_id, assignee_agent_id, review_round, artifact_path \
             FROM tasks WHERE id = ?",
        )
        .bind(task_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| ("sql".to_string(), e.to_string()))?;
    let (status, startup_id, parent_id, assignee, review_round, artifact_path) =
        row.ok_or_else(|| ("unknown_task".to_string(), format!("no task: {task_id}")))?;
    let status = parse_status(&status)
        .ok_or_else(|| ("unknown_status".to_string(), status.clone()))?;
    Ok(TaskRow {
        status,
        startup_id,
        parent_id,
        assignee_agent_id: assignee,
        review_round: review_round as u32,
        artifact_path,
    })
}

struct TaskRow {
    status: TaskStatus,
    startup_id: String,
    parent_id: Option<String>,
    assignee_agent_id: Option<String>,
    review_round: u32,
    artifact_path: Option<String>,
}

fn parse_status(s: &str) -> Option<TaskStatus> {
    match s {
        "proposed" => Some(TaskStatus::Proposed),
        "queued" => Some(TaskStatus::Queued),
        "in_progress" => Some(TaskStatus::InProgress),
        "awaiting_review" => Some(TaskStatus::AwaitingReview),
        "changes_requested" => Some(TaskStatus::ChangesRequested),
        "done" => Some(TaskStatus::Done),
        "failed" => Some(TaskStatus::Failed),
        "escalated" => Some(TaskStatus::Escalated),
        _ => None,
    }
}

fn status_to_str(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Proposed => "proposed",
        TaskStatus::Queued => "queued",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::AwaitingReview => "awaiting_review",
        TaskStatus::ChangesRequested => "changes_requested",
        TaskStatus::Done => "done",
        TaskStatus::Failed => "failed",
        TaskStatus::Escalated => "escalated",
    }
}

/// Phase 0 manager check for a task: caller is the parent task's assignee
/// (subtask manager) or, for root tasks, the assignee's recorded `manager_id`.
async fn caller_is_manager_of_task(
    pool: &SqlitePool,
    caller: &AvatarView,
    task: &TaskRow,
) -> Result<bool, (String, String)> {
    if let Some(parent_id) = &task.parent_id {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT assignee_agent_id FROM tasks WHERE id = ?")
                .bind(parent_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| ("sql".to_string(), e.to_string()))?;
        return Ok(row
            .and_then(|(a,)| a)
            .map(|a| a == caller.agent_id)
            .unwrap_or(false));
    }
    // Root task: manager is the assignee's `agents.manager_id`.
    let assignee = match &task.assignee_agent_id {
        Some(a) => a.clone(),
        None => return Ok(false),
    };
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT manager_id FROM agents WHERE id = ?")
            .bind(&assignee)
            .fetch_optional(pool)
            .await
            .map_err(|e| ("sql".to_string(), e.to_string()))?;
    Ok(row
        .and_then(|(m,)| m)
        .map(|m| m == caller.agent_id)
        .unwrap_or(false))
}

/// True when the caller's `agent_id` equals `agents.manager_id` for the parent
/// task's assignee — i.e., the caller manages the subtask's siblings. Used by
/// `subtask_create` to decide queued-vs-proposed without a transition.
async fn caller_manages_parent(
    pool: &SqlitePool,
    caller: &AvatarView,
    parent_id: &str,
) -> Result<bool, (String, String)> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT assignee_agent_id FROM tasks WHERE id = ?")
            .bind(parent_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| ("sql".to_string(), e.to_string()))?;
    Ok(row
        .and_then(|(a,)| a)
        .map(|a| a == caller.agent_id)
        .unwrap_or(false))
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, (String, String)> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ("bad_args".to_string(), format!("{key} required")))
}

/// Public rooms (spec invariant 7): lobby, cafe, library are shared social
/// spaces where chat broadcasts cross startup boundaries. Suites remain
/// private (same-startup-only). Phase 0 keeps this list inline; Phase 1 may
/// derive it from `rooms.type` (`social`/`transit`/`focus`) or an explicit
/// `is_public` column.
fn is_public_room(room_id: &str) -> bool {
    matches!(room_id, "lobby" | "cafe" | "library")
}

// ── handlers ────────────────────────────────────────────────────────────────

async fn handle_move_intent(
    world: &mut WorldView,
    paths: &mut PathStore,
    layout: &TownLayout,
    graph: &RoomGraph,
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let target_room = args.get("target_room").and_then(|v| v.as_str());
    let target_tile = args.get("target_tile");
    let (room, x, y) = match (target_room, target_tile) {
        (Some(r), Some(t)) => {
            let x = t
                .get("x")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ("bad_args".to_string(), "target_tile.x missing".to_string()))?
                as i32;
            let y = t
                .get("y")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ("bad_args".to_string(), "target_tile.y missing".to_string()))?
                as i32;
            (r.to_string(), x, y)
        }
        (Some(r), None) => {
            // Default to room center when only a room was supplied.
            let room_def = layout
                .room(r)
                .ok_or_else(|| ("bad_args".to_string(), format!("unknown room: {r}")))?;
            let cx = room_def.bounds.0 + room_def.bounds.2 / 2;
            let cy = room_def.bounds.1 + room_def.bounds.3 / 2;
            (r.to_string(), cx, cy)
        }
        (None, Some(t)) => {
            // Codex round-5 P2#3: tile-only — move within the caller's
            // current room. The spec allows omitting `target_room` when the
            // agent just wants to walk to a different tile in the same room.
            let x = t
                .get("x")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ("bad_args".to_string(), "target_tile.x missing".to_string()))?
                as i32;
            let y = t
                .get("y")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ("bad_args".to_string(), "target_tile.y missing".to_string()))?
                as i32;
            (caller.room_id.clone(), x, y)
        }
        _ => {
            return Err((
                "bad_args".to_string(),
                "must provide target_room or target_tile".to_string(),
            ))
        }
    };

    match move_sys::start_move(world, paths, layout, graph, &caller.agent_id, &room, x, y) {
        StartMoveResult::Ok => Ok(json!({
            "target_room": room,
            "target_tile": {"x": x, "y": y},
            // The arrival ETA is the path length in ticks; for Phase 0 the
            // caller can poll its `move_complete` event instead. We surface
            // the queued path length when available.
            "queued_steps": paths
                .get(&caller.agent_id)
                .map(|p| p.len() as u64)
                .unwrap_or(0),
        })),
        StartMoveResult::NoPath => Err(("no_path".into(), "no path to target".into())),
        StartMoveResult::PermissionDenied => {
            // M5.7: cross-startup room access is a permission violation. The
            // mcp_error keeps the wire contract; the system_events alert row
            // gives the operator console a durable audit trail of who tried
            // to enter what so we can spot misbehaving agents over time.
            if let Err(e) = crate::emit::emit_system_event(
                pool,
                event_tx,
                Some(&caller.startup_id),
                "permission_violation",
                &json!({
                    "agent_id": caller.agent_id,
                    "kind": "move_intent_denied",
                    "target_room": room,
                })
                .to_string(),
                "alert",
            )
            .await
            {
                tracing::error!(component = "mcp_dispatch", agent_id = %caller.agent_id, err = %e, "failed to emit permission_violation system_event");
            }
            Err((
                "no_permission".into(),
                "cannot enter target room".into(),
            ))
        }
        StartMoveResult::NoSuchAgent => Err(("unknown_agent".into(), "agent not found".into())),
    }
}

async fn handle_speak(
    world: &WorldView,
    out_bus: &HashMap<String, mpsc::Sender<Value>>,
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let body = require_str(&args, "body")?.to_string();
    check_body_length("body", &body)?;
    let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("chat");
    let to_agent_id = args
        .get("to_agent_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if kind != "chat" && kind != "directive" {
        return Err(("bad_args".into(), format!("unknown speak kind: {kind}")));
    }

    if kind == "directive" {
        let recipient = to_agent_id
            .as_deref()
            .ok_or_else(|| ("bad_args".to_string(), "directive requires to_agent_id".to_string()))?;
        // Phase 0 org-graph: caller must manage the recipient. Same-startup is
        // a precondition of `manager_id`, so a hit here implies same-startup.
        let row: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT startup_id, manager_id FROM agents WHERE id = ?")
                .bind(recipient)
                .fetch_optional(pool)
                .await
                .map_err(|e| ("sql".to_string(), e.to_string()))?;
        let (rec_startup, rec_manager) = row
            .ok_or_else(|| ("unknown_recipient".to_string(), format!("no such agent: {recipient}")))?;
        if rec_startup != caller.startup_id {
            return Err(("cross_startup".into(), "directive cannot cross startups".into()));
        }
        if rec_manager.as_deref() != Some(&caller.agent_id) {
            return Err((
                "no_permission".into(),
                "directive requires manager-of-recipient".into(),
            ));
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    // Persist the message. `chat` carries `room_id`; `directive` is room-
    // independent so we leave room_id NULL (matches the operator-directive
    // path in cmd_console).
    let room_for_row = if kind == "chat" {
        Some(caller.room_id.as_str())
    } else {
        None
    };
    sqlx::query(
        "INSERT INTO messages (id, startup_id, room_id, author_id, body, kind, ts) \
         VALUES (?, ?, ?, ?, ?, ?, unixepoch())",
    )
    .bind(&id)
    .bind(&caller.startup_id)
    .bind(room_for_row)
    .bind(&caller.agent_id)
    .bind(&body)
    .bind(kind)
    .execute(pool)
    .await
    .map_err(|e| ("sql".to_string(), e.to_string()))?;

    if kind == "chat" {
        // Spec invariant 7: chat is room-scoped, not org-scoped, and public
        // rooms (lobby/cafe/library) fan across startups so visitors can
        // overhear each other. Private suites stay same-startup-only.
        let public = is_public_room(&caller.room_id);
        for (peer_id, peer) in &world.avatars {
            if peer_id == &caller.agent_id {
                continue;
            }
            if peer.room_id != caller.room_id {
                continue;
            }
            let same_startup = peer.startup_id == caller.startup_id;
            if !(same_startup || public) {
                continue;
            }
            if let Some(tx) = out_bus.get(peer_id) {
                let _ = tx.try_send(json!({
                    "type":"chat_received","v":1,
                    "from_agent_id":caller.agent_id,
                    "body":body,
                    "room_id":caller.room_id
                }));
            }
        }
        // Broadcast a Chat frame to operator consoles (god view).
        let _ = event_tx.send(crate::protocol::ConsoleOutbound::Chat {
            v: 1,
            message_id: id.clone(),
            ts: chrono::Utc::now().timestamp_millis(),
            startup_id: caller.startup_id.clone(),
            room_id: caller.room_id.clone(),
            author_id: caller.agent_id.clone(),
            body: body.clone(),
        });
    } else if let Some(rid) = to_agent_id.as_deref() {
        if let Some(tx) = out_bus.get(rid) {
            let _ = tx.try_send(json!({
                "type":"directive","v":1,
                "from_agent_id":caller.agent_id,
                "body":body
            }));
        }
        // Broadcast a Directive frame to operator consoles (god view).
        // rid is non-empty because the early-validate above returns Err otherwise.
        let _ = event_tx.send(crate::protocol::ConsoleOutbound::Directive {
            v: 1,
            message_id: id.clone(),
            ts: chrono::Utc::now().timestamp_millis(),
            startup_id: caller.startup_id.clone(),
            author_id: caller.agent_id.clone(),
            to_agent_id: rid.to_string(),
            body: body.clone(),
            in_response_to_task: None,
        });
    }

    Ok(json!({"message_id": id}))
}

async fn handle_task_done(
    world: &mut WorldView,
    out_bus: &HashMap<String, mpsc::Sender<Value>>,
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let task_id = require_str(&args, "task_id")?.to_string();
    let artifact_path = require_str(&args, "artifact_path")?.to_string();

    let task = load_task(pool, &task_id).await?;
    if task.startup_id != caller.startup_id {
        return Err(("cross_startup".into(), "task belongs to another startup".into()));
    }
    if task.assignee_agent_id.as_deref() != Some(caller.agent_id.as_str()) {
        return Err(("no_permission".into(), "task_done requires assignee".into()));
    }

    // Spec §5.4: artifact path must be exactly
    // `workspaces/<startup_id>/artifacts/<task_id>.md`. Fast-fail on the
    // canonical pattern *before* sandbox::resolve so we never canonicalize a
    // path that doesn't match the contract; sandbox::resolve still runs as
    // belt-and-suspenders to defeat any symlink/traversal trick that could
    // otherwise sneak past a string compare.
    let canonical = format!(
        "workspaces/{}/artifacts/{}.md",
        caller.startup_id, task_id
    );
    if artifact_path != canonical {
        return Err((
            "bad_artifact_path".into(),
            format!("expected {canonical}, got {artifact_path}"),
        ));
    }

    // The canonical pattern is rooted at `workspaces/<startup_id>/`, so the
    // path we hand sandbox::resolve is the artifacts/<task_id>.md tail. This
    // also matches the per-startup workspace_root used by every other tool.
    let workspace_root = std::path::PathBuf::from(format!("workspaces/{}", caller.startup_id));
    let inside = format!("artifacts/{task_id}.md");
    sandbox::resolve(&workspace_root, &inside)
        .map_err(|e| ("sandbox_violation".to_string(), format!("{e}")))?;

    let new_status = next(task.status, &Transition::TaskDoneMcp)
        .map_err(|r| ("illegal_transition".to_string(), r.to_string()))?;
    sqlx::query(
        "UPDATE tasks SET status = ?, artifact_path = ?, updated_at = unixepoch() WHERE id = ?",
    )
    .bind(status_to_str(new_status))
    .bind(&artifact_path)
    .bind(&task_id)
    .execute(pool)
    .await
    .map_err(|e| ("sql".to_string(), e.to_string()))?;
    let _ = persist::append_audit(
        pool,
        &task_id,
        &json!({"actor":"agent","kind":"task_done","agent_id":caller.agent_id}).to_string(),
    )
    .await;

    // Mirror the inverse of `scheduler::tick`'s `idle → working` flip. Without
    // this, the avatar stays `working` after completion and the scheduler
    // refuses to dispatch any further queued task to the same agent — so each
    // agent runs exactly one task in its lifetime. Phase 1 will route status
    // through the worker's own status_changed event so multi-step internal
    // work doesn't keep blocking dispatch; Phase 0 ties it directly to
    // task_done/task_failed.
    if let Some(av) = world.avatars.get_mut(&caller.agent_id) {
        av.status = "idle".to_string();
    }

    // Notify the manager via subtask_done (parent's assignee). The plan calls
    // out grand-manager propagation as M3+ work — we log a TODO when the
    // chain depth > 1 and emit only one hop here.
    if let Some(parent_id) = &task.parent_id {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT assignee_agent_id FROM tasks WHERE id = ?")
                .bind(parent_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| ("sql".to_string(), e.to_string()))?;
        if let Some(manager) = row.and_then(|(a,)| a) {
            if let Some(tx) = out_bus.get(&manager) {
                let _ = tx.try_send(json!({
                    "type":"subtask_done","v":1,
                    "parent_id": parent_id,
                    "child_id": task_id,
                    "artifact_path": artifact_path,
                    "review_round": task.review_round,
                }));
            }
        }
    }

    Ok(json!({"task_id": task_id, "new_status": status_to_str(new_status)}))
}

async fn handle_task_failed(
    world: &mut WorldView,
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let task_id = require_str(&args, "task_id")?.to_string();
    let reason = args
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let task = load_task(pool, &task_id).await?;
    if task.startup_id != caller.startup_id {
        return Err(("cross_startup".into(), "task belongs to another startup".into()));
    }
    if task.assignee_agent_id.as_deref() != Some(caller.agent_id.as_str()) {
        return Err(("no_permission".into(), "task_failed requires assignee".into()));
    }
    let new_status = next(task.status, &Transition::Fail)
        .map_err(|r| ("illegal_transition".to_string(), r.to_string()))?;
    sqlx::query("UPDATE tasks SET status = ?, updated_at = unixepoch() WHERE id = ?")
        .bind(status_to_str(new_status))
        .bind(&task_id)
        .execute(pool)
        .await
        .map_err(|e| ("sql".to_string(), e.to_string()))?;
    let _ = persist::append_audit(
        pool,
        &task_id,
        &json!({"actor":"agent","kind":"task_failed","agent_id":caller.agent_id,"reason":reason})
            .to_string(),
    )
    .await;
    // Inverse of `scheduler::tick`'s `idle → working` flip. See the parallel
    // comment in `handle_task_done` for context: without this, the agent stays
    // `working` forever and never picks up its next queued task.
    if let Some(av) = world.avatars.get_mut(&caller.agent_id) {
        av.status = "idle".to_string();
    }
    Ok(json!({"task_id": task_id, "new_status": status_to_str(new_status)}))
}

async fn handle_subtask_create(
    out_bus: &HashMap<String, mpsc::Sender<Value>>,
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let parent_id = require_str(&args, "parent_id")?.to_string();
    let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let required_room = args
        .get("required_room")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let requested_assignee = args
        .get("assignee_agent_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Same-startup check on the parent task before we decide caller_manages.
    let parent: (String, Option<String>) =
        sqlx::query_as("SELECT startup_id, assignee_agent_id FROM tasks WHERE id = ?")
            .bind(&parent_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| ("sql".to_string(), e.to_string()))?
            .ok_or_else(|| ("unknown_task".to_string(), format!("no parent: {parent_id}")))?;
    if parent.0 != caller.startup_id {
        return Err((
            "cross_startup".into(),
            "parent task belongs to another startup".into(),
        ));
    }

    let is_manager = parent.1.as_deref() == Some(caller.agent_id.as_str());
    let actor = if is_manager { Actor::Manager } else { Actor::NonManager };
    let new_status = next(TaskStatus::Proposed /* unused */, &Transition::SubtaskCreate { caller: actor })
        .map_err(|r| ("illegal_transition".to_string(), r.to_string()))?;

    // Non-managers can't pick the assignee — null it out (spec §6.2).
    let assignee = if is_manager { requested_assignee } else { None };
    // Codex round-5 P1#2: when the manager picks an explicit assignee, that
    // agent must belong to the caller's startup. Same bug class as round-3
    // P2#4 (accept_proposal) — without this gate the scheduler would dispatch
    // the foreign task to the wrong-startup worker, and `task_done` would
    // later reject it cross-startup, wedging the task forever.
    if let Some(aid) = assignee.as_deref() {
        let r: Option<(String,)> =
            sqlx::query_as("SELECT startup_id FROM agents WHERE id = ?")
                .bind(aid)
                .fetch_optional(pool)
                .await
                .map_err(|e| ("sql".to_string(), e.to_string()))?;
        let assignee_sid = match r {
            Some((s,)) => s,
            None => {
                return Err((
                    "unknown_assignee".into(),
                    "no such agent".into(),
                ));
            }
        };
        if assignee_sid != caller.startup_id {
            return Err((
                "cross_startup".into(),
                "assignee in different startup".into(),
            ));
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, parent_id, title, description, status, \
         assignee_agent_id, required_room, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, unixepoch(), unixepoch())",
    )
    .bind(&id)
    .bind(&caller.startup_id)
    .bind(&parent_id)
    .bind(&title)
    .bind(&description)
    .bind(status_to_str(new_status))
    .bind(&assignee)
    .bind(&required_room)
    .execute(pool)
    .await
    .map_err(|e| ("sql".to_string(), e.to_string()))?;

    // For non-managers, notify the parent's assignee (the manager) via
    // subtask_proposed; managers go straight to queued and the scheduler
    // takes over.
    if !is_manager {
        if let Some(manager) = parent.1.as_deref() {
            if let Some(tx) = out_bus.get(manager) {
                let _ = tx.try_send(json!({
                    "type":"subtask_proposed","v":1,
                    "parent_id": parent_id,
                    "proposed_task_id": id,
                    "proposer_agent_id": caller.agent_id,
                    "title": title,
                    "description": description,
                }));
            }
        }
    }

    Ok(json!({"task_id": id, "status": status_to_str(new_status)}))
}

async fn handle_task_accept(
    out_bus: &HashMap<String, mpsc::Sender<Value>>,
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let task_id = require_str(&args, "task_id")?.to_string();
    let task = load_task(pool, &task_id).await?;
    if task.startup_id != caller.startup_id {
        return Err(("cross_startup".into(), "task belongs to another startup".into()));
    }
    if !caller_is_manager_of_task(pool, caller, &task).await? {
        return Err(("no_permission".into(), "task_accept is manager-only".into()));
    }
    let new_status = next(task.status, &Transition::TaskAccept)
        .map_err(|r| ("illegal_transition".to_string(), r.to_string()))?;
    sqlx::query("UPDATE tasks SET status = ?, updated_at = unixepoch() WHERE id = ?")
        .bind(status_to_str(new_status))
        .bind(&task_id)
        .execute(pool)
        .await
        .map_err(|e| ("sql".to_string(), e.to_string()))?;
    let _ = persist::append_audit(
        pool,
        &task_id,
        &json!({"actor":"manager","kind":"task_accept","agent_id":caller.agent_id}).to_string(),
    )
    .await;

    // Spec asks for grand-manager propagation when this task itself has a
    // parent; Phase 0 emits one hop and TODO-logs deeper chains so the
    // semantics of multi-level rollup land in M3+.
    if let Some(parent_id) = &task.parent_id {
        let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT assignee_agent_id, parent_id FROM tasks WHERE id = ?",
        )
        .bind(parent_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| ("sql".to_string(), e.to_string()))?;
        if let Some((manager, grandparent)) = row {
            if let Some(m) = manager.as_deref() {
                if let Some(tx) = out_bus.get(m) {
                    let _ = tx.try_send(json!({
                        "type":"subtask_done","v":1,
                        "parent_id": parent_id,
                        "child_id": task_id,
                        "artifact_path": task.artifact_path.clone().unwrap_or_default(),
                        "review_round": task.review_round,
                    }));
                }
            }
            if grandparent.is_some() {
                tracing::warn!(component = "mcp_dispatch",
                    task_id = %task_id,
                    "task_accept: grand-manager propagation deferred to M3+"
                );
            }
        }
    }

    Ok(json!({"task_id": task_id, "new_status": status_to_str(new_status)}))
}

async fn handle_task_request_changes(
    out_bus: &HashMap<String, mpsc::Sender<Value>>,
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let task_id = require_str(&args, "task_id")?.to_string();
    let feedback = require_str(&args, "feedback")?.to_string();
    check_body_length("feedback", &feedback)?;
    let task = load_task(pool, &task_id).await?;
    if task.startup_id != caller.startup_id {
        return Err(("cross_startup".into(), "task belongs to another startup".into()));
    }
    // P4 E1: peer-review path. Permission check is now an OR:
    //   - caller is the manager-of-task (legacy path), OR
    //   - caller is a peer reviewer (`agents.is_peer_reviewer = 1`) in the
    //     same startup AND is NOT the assignee (no self-review).
    // The audit_trail entry carries the discriminator so the org graph
    // stays inferable post-hoc.
    let is_manager = caller_is_manager_of_task(pool, caller, &task).await?;
    let is_peer_review = if !is_manager {
        let same_startup = task.startup_id == caller.startup_id; // re-affirm (already checked above)
        let not_self = task.assignee_agent_id.as_deref() != Some(caller.agent_id.as_str());
        let peer_row: Option<(i64,)> =
            sqlx::query_as("SELECT is_peer_reviewer FROM agents WHERE id = ?")
                .bind(&caller.agent_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| ("sql".into(), e.to_string()))?;
        let is_peer = peer_row.map(|(v,)| v != 0).unwrap_or(false);
        same_startup && not_self && is_peer
    } else {
        false
    };
    if !is_manager && !is_peer_review {
        return Err((
            "no_permission".into(),
            "task_request_changes requires manager or peer-reviewer role".into(),
        ));
    }
    let actor_kind = if is_manager { "manager" } else { "peer" };

    // Guard: must run BEFORE any side effects.
    // A subtask whose parent.assignee = caller passes the manager check above,
    // but if the subtask itself has no assignee, the directive has nowhere to go.
    // Reject early so the task state stays clean.
    if task.assignee_agent_id.is_none() {
        return Err(("no_assignee".into(), "task has no assignee".into()));
    }

    // Escalation branch: max-rounds breach. NO directive INSERT, NO Directive
    // broadcast. Emits a single SystemEvent via emit_system_event so the
    // operator console sees the task transition to escalated.
    if task.review_round >= MAX_REVIEW_ROUNDS {
        let escalated = next(task.status, &Transition::Escalate)
            .map_err(|r| ("illegal_transition".into(), r.to_string()))?;
        sqlx::query("UPDATE tasks SET status = ?, updated_at = unixepoch() WHERE id = ?")
            .bind(status_to_str(escalated)).bind(&task_id)
            .execute(pool).await
            .map_err(|e| ("sql".into(), e.to_string()))?;
        let _ = persist::append_audit(
            pool, &task_id,
            &json!({
                "actor":"system","kind":"escalated",
                "reason":"max_review_rounds_exceeded",
                "at_round":task.review_round,
                "triggered_by":caller.agent_id,
            }).to_string(),
        ).await;
        if let Err(e) = crate::emit::emit_system_event(
            pool, event_tx,
            Some(&caller.startup_id),
            "task_escalated",
            &json!({
                "task_id": task_id,
                "rounds": task.review_round,
                "feedback": feedback,
            }).to_string(),
            "alert",
        ).await {
            tracing::error!(component = "mcp_dispatch",
                task_id = %task_id,
                err = %e,
                "failed to emit task_escalated system_event after task UPDATE committed"
            );
        }
        return Ok(json!({
            "task_id": task_id,
            "new_status": status_to_str(escalated),
            "status": status_to_str(escalated),
            "review_round": task.review_round,
            "reason": "max_review_rounds_exceeded",
        }));
    }

    // Regular round-increment branch: task UPDATE + directive INSERT in a
    // single transaction so the broadcast-after-SQL invariant holds.
    let new_status = next(task.status, &Transition::RequestChanges)
        .map_err(|r| ("illegal_transition".into(), r.to_string()))?;
    let directive_id = uuid::Uuid::new_v4().to_string();
    let mut tx = pool.begin().await.map_err(|e| ("sql".into(), e.to_string()))?;
    sqlx::query(
        "UPDATE tasks SET status = ?, review_round = review_round + 1, updated_at = unixepoch() WHERE id = ?",
    )
    .bind(status_to_str(new_status)).bind(&task_id)
    .execute(&mut *tx).await
    .map_err(|e| ("sql".into(), e.to_string()))?;
    sqlx::query(
        "INSERT INTO messages (id, startup_id, room_id, author_id, body, kind, ts) \
         VALUES (?, ?, NULL, ?, ?, 'directive', unixepoch())",
    )
    .bind(&directive_id).bind(&caller.startup_id).bind(&caller.agent_id).bind(&feedback)
    .execute(&mut *tx).await
    .map_err(|e| ("sql".into(), e.to_string()))?;
    tx.commit().await.map_err(|e| ("sql".into(), e.to_string()))?;

    let _ = persist::append_audit(
        pool, &task_id,
        &json!({"actor":actor_kind,"kind":"task_request_changes","agent_id":caller.agent_id}).to_string(),
    ).await;

    // assignee is guaranteed non-None by the guard above.
    let assignee = task.assignee_agent_id.as_deref().expect("checked above for None");
    let _ = event_tx.send(crate::protocol::ConsoleOutbound::Directive {
        v: 1,
        message_id: directive_id,
        ts: chrono::Utc::now().timestamp_millis(),
        startup_id: caller.startup_id.clone(),
        author_id: caller.agent_id.clone(),
        to_agent_id: assignee.to_string(),
        body: feedback.clone(),
        in_response_to_task: Some(task_id.clone()),
    });
    if let Some(tx) = out_bus.get(assignee) {
        let _ = tx.try_send(json!({
            "type":"directive","v":1,
            "from_agent_id": caller.agent_id,
            "body": feedback,
            "in_response_to_task": task_id,
        }));
    }

    Ok(json!({"task_id": task_id, "new_status": status_to_str(new_status)}))
}

async fn handle_accept_proposal(
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let task_id = require_str(&args, "task_id")?.to_string();
    let assignee = require_str(&args, "assignee_agent_id")?.to_string();
    let required_room = args
        .get("required_room")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let task = load_task(pool, &task_id).await?;
    if task.startup_id != caller.startup_id {
        return Err(("cross_startup".into(), "task belongs to another startup".into()));
    }
    let parent_id = task
        .parent_id
        .as_deref()
        .ok_or_else(|| ("bad_args".to_string(), "accept_proposal requires a subtask".to_string()))?;
    if !caller_manages_parent(pool, caller, parent_id).await? {
        return Err((
            "no_permission".into(),
            "accept_proposal is manager-only".into(),
        ));
    }
    // Codex round-3 P2#4: assignee must belong to the same startup as the
    // task. Without this check the scheduler dispatches the foreign task to
    // the wrong-startup worker; `task_done` then rejects it as cross-startup,
    // leaving the task wedged in `queued`.
    let assignee_startup: Option<(String,)> =
        sqlx::query_as("SELECT startup_id FROM agents WHERE id = ?")
            .bind(&assignee)
            .fetch_optional(pool)
            .await
            .map_err(|e| ("sql".to_string(), e.to_string()))?;
    let assignee_sid = match assignee_startup {
        Some((s,)) => s,
        None => return Err(("unknown_assignee".into(), "no such agent".into())),
    };
    if assignee_sid != caller.startup_id {
        return Err((
            "cross_startup".into(),
            "assignee in different startup".into(),
        ));
    }
    let new_status = next(task.status, &Transition::AcceptProposal { caller: Actor::Manager })
        .map_err(|r| ("illegal_transition".to_string(), r.to_string()))?;
    sqlx::query(
        "UPDATE tasks SET status = ?, assignee_agent_id = ?, required_room = ?, updated_at = unixepoch() WHERE id = ?",
    )
    .bind(status_to_str(new_status))
    .bind(&assignee)
    .bind(&required_room)
    .bind(&task_id)
    .execute(pool)
    .await
    .map_err(|e| ("sql".to_string(), e.to_string()))?;
    let _ = persist::append_audit(
        pool,
        &task_id,
        &json!({"actor":"manager","kind":"accept_proposal","agent_id":caller.agent_id}).to_string(),
    )
    .await;
    Ok(json!({"task_id": task_id, "new_status": status_to_str(new_status)}))
}

async fn handle_reject_proposal(
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let task_id = require_str(&args, "task_id")?.to_string();
    let reason = args.get("reason").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let task = load_task(pool, &task_id).await?;
    if task.startup_id != caller.startup_id {
        return Err(("cross_startup".into(), "task belongs to another startup".into()));
    }
    let parent_id = task
        .parent_id
        .as_deref()
        .ok_or_else(|| ("bad_args".to_string(), "reject_proposal requires a subtask".to_string()))?;
    if !caller_manages_parent(pool, caller, parent_id).await? {
        return Err((
            "no_permission".into(),
            "reject_proposal is manager-only".into(),
        ));
    }
    let new_status = next(task.status, &Transition::RejectProposal { caller: Actor::Manager })
        .map_err(|r| ("illegal_transition".to_string(), r.to_string()))?;
    sqlx::query("UPDATE tasks SET status = ?, updated_at = unixepoch() WHERE id = ?")
        .bind(status_to_str(new_status))
        .bind(&task_id)
        .execute(pool)
        .await
        .map_err(|e| ("sql".to_string(), e.to_string()))?;
    let _ = persist::append_audit(
        pool,
        &task_id,
        &json!({"actor":"manager","kind":"reject_proposal","agent_id":caller.agent_id,"reason":reason})
            .to_string(),
    )
    .await;
    Ok(json!({"task_id": task_id, "new_status": status_to_str(new_status)}))
}

async fn handle_epistemic_append(
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
    kind: &str,
) -> HandlerResult {
    let task_id = require_str(&args, "task_id")?.to_string();
    let task = load_task(pool, &task_id).await?;
    if task.startup_id != caller.startup_id {
        return Err(("cross_startup".into(), "task belongs to another startup".into()));
    }
    if task.assignee_agent_id.as_deref() != Some(caller.agent_id.as_str()) {
        return Err(("no_permission".into(), "epistemic append requires assignee".into()));
    }
    // Inject the kind tag and timestamp so consumers don't need to guess
    // which call type produced the entry.
    let mut entry = match args {
        Value::Object(map) => Value::Object(map),
        _ => return Err(("bad_args".into(), "args must be an object".into())),
    };
    if let Some(obj) = entry.as_object_mut() {
        obj.insert("kind".to_string(), json!(kind));
        obj.insert("agent_id".to_string(), json!(caller.agent_id));
    }
    persist::append_epistemic(pool, &task_id, &entry.to_string())
        .await
        .map_err(|e| ("sql".to_string(), e.to_string()))?;
    Ok(json!({"task_id": task_id, "kind": kind}))
}

async fn handle_verify(caller: &AvatarView, args: Value) -> HandlerResult {
    let method = require_str(&args, "method")?.to_string();
    let params = args.get("params").cloned().unwrap_or(Value::Null);
    match method.as_str() {
        "read_assert" => {
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ("bad_args".to_string(), "params.path required".to_string()))?;
            let needle = params.get("contains").and_then(|v| v.as_str());
            let workspace_root =
                std::path::PathBuf::from(format!("workspaces/{}", caller.startup_id));
            let resolved = sandbox::resolve(&workspace_root, path)
                .map_err(|e| ("sandbox_violation".to_string(), format!("{e}")))?;
            let content = tokio::fs::read_to_string(&resolved)
                .await
                .map_err(|e| ("read_failed".to_string(), e.to_string()))?;
            let ok = match needle {
                Some(s) => content.contains(s),
                // No predicate supplied = "file readable" — useful for
                // existence assertions.
                None => true,
            };
            Ok(json!({"observed": {"ok": ok, "len": content.len()}}))
        }
        "lint_json" => {
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ("bad_args".to_string(), "params.path required".to_string()))?;
            let workspace_root =
                std::path::PathBuf::from(format!("workspaces/{}", caller.startup_id));
            let resolved = sandbox::resolve(&workspace_root, path)
                .map_err(|e| ("sandbox_violation".to_string(), format!("{e}")))?;
            let content = tokio::fs::read_to_string(&resolved)
                .await
                .map_err(|e| ("read_failed".to_string(), e.to_string()))?;
            match serde_json::from_str::<Value>(&content) {
                Ok(_) => Ok(json!({"observed": {"ok": true}})),
                Err(e) => Ok(json!({"observed": {"ok": false, "error": e.to_string()}})),
            }
        }
        "lint_markdown" | "lint_typescript" => {
            // Phase 0 stub; spec §6.4 routes these through the TS sidecar.
            Ok(json!({"observed": {"ok": true, "deferred": true, "method": method}}))
        }
        _ => Err(("bad_args".into(), format!("unknown verify method: {method}"))),
    }
}

async fn handle_ask_peer(_args: Value) -> HandlerResult {
    // Phase 0 stub: no peer round-trip until M3+ wires the response listener.
    // The CLI receives a deterministic null instead of blocking on a timer
    // that no peer would respond to.
    Ok(json!({"response": null}))
}

async fn handle_observe_world(
    world: &WorldView,
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let query = require_str(&args, "query")?;
    match query {
        "peers_in_room" => {
            let peers: Vec<Value> = world
                .avatars
                .values()
                .filter(|a| {
                    a.room_id == caller.room_id
                        && a.agent_id != caller.agent_id
                        // Same-startup gate: chat-style peer visibility is
                        // intra-startup only.
                        && a.startup_id == caller.startup_id
                })
                .map(|a| json!({"agent_id": a.agent_id, "role": a.role}))
                .collect();
            Ok(json!({"peers": peers}))
        }
        "my_position" => Ok(json!({
            "room_id": caller.room_id,
            "tile": {"x": caller.current_pos.0, "y": caller.current_pos.1},
        })),
        "budget_remaining" => {
            let row: Option<(f64, f64)> = sqlx::query_as(
                "SELECT budget_spent_usd, budget_cap_usd FROM startups WHERE id = ?",
            )
            .bind(&caller.startup_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| ("sql".to_string(), e.to_string()))?;
            match row {
                Some((spent, cap)) => Ok(json!({
                    "spent_usd": spent,
                    "cap_usd": cap,
                    "remaining_usd": (cap - spent).max(0.0),
                })),
                None => Err(("unknown_startup".into(), "startup not found".into())),
            }
        }
        other => Err(("bad_args".into(), format!("unknown query: {other}"))),
    }
}

async fn handle_read_artifact(caller: &AvatarView, args: Value) -> HandlerResult {
    let path = require_str(&args, "path")?.to_string();
    // Same-startup gate baked into the path: each startup's workspace lives
    // at workspaces/<startup_id>/, and `sandbox::resolve` rejects anything
    // that escapes that root. This is the §6.2 "same-startup only" rule.
    let workspace_root = std::path::PathBuf::from(format!("workspaces/{}", caller.startup_id));
    let resolved = sandbox::resolve(&workspace_root, &path)
        .map_err(|e| ("sandbox_violation".to_string(), format!("{e}")))?;
    let content = tokio::fs::read_to_string(&resolved)
        .await
        .map_err(|e| ("read_failed".to_string(), e.to_string()))?;
    Ok(json!({"path": path, "content": content}))
}

async fn handle_skill_upsert(
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let name = require_str(&args, "name")?.to_string();
    let content_md = require_str(&args, "content_md")?.to_string();
    match crate::skills::upsert_with_author(
        pool,
        &caller.startup_id,
        &name,
        &content_md,
        crate::skills::Author::Agent(&caller.agent_id),
    )
    .await
    {
        Ok((id, created)) => {
            // P2.2 broadcast: re-fetch the listing row so frontend can apply
            // in place without a follow-up snapshot.
            if let Ok(rows) =
                crate::skills::list_with_attachments(pool, &caller.startup_id).await
            {
                let skill_json = rows
                    .iter()
                    .find(|s| s.id == id)
                    .map(crate::skills::skill_with_attachments_to_json);
                let _ = event_tx.send(crate::protocol::ConsoleOutbound::SkillChanged {
                    v: 1,
                    startup_id: caller.startup_id.clone(),
                    kind: "upsert".to_string(),
                    skill_id: id.clone(),
                    agent_id: None,
                    skill: skill_json,
                });
            }
            Ok(json!({"id": id, "created": created}))
        }
        Err(crate::skills::SkillError::BadName) => Err((
            "bad_skill_name".into(),
            format!("name must match [A-Za-z0-9_-]{{1,64}}; got {name:?}"),
        )),
        Err(crate::skills::SkillError::OversizeContent) => Err((
            "skill_content_too_long".into(),
            format!("content_md exceeds {} bytes", crate::skills::MAX_CONTENT_LEN),
        )),
        Err(crate::skills::SkillError::NotFound) => {
            Err(("not_found".into(), "skill row vanished mid-upsert".into()))
        }
        Err(crate::skills::SkillError::CrossStartup) => {
            Err(("cross_startup".into(), "caller can't author skill in another startup".into()))
        }
        Err(crate::skills::SkillError::Sql(e)) => Err(("sql".into(), e)),
    }
}

async fn handle_skill_list(
    pool: &SqlitePool,
    caller: &AvatarView,
    _args: Value,
) -> HandlerResult {
    let items = crate::skills::list(pool, &caller.startup_id)
        .await
        .map_err(|e| match e {
            crate::skills::SkillError::Sql(s) => ("sql".to_string(), s),
            _ => ("sql".to_string(), "unexpected error".to_string()),
        })?;
    let arr: Vec<Value> = items
        .into_iter()
        .map(|s| {
            json!({
                "id": s.id,
                "name": s.name,
                "updated_at": s.updated_at,
                "len": s.len,
            })
        })
        .collect();
    Ok(json!({"skills": arr}))
}

async fn handle_skill_attach(
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let agent_id = require_str(&args, "agent_id")?.to_string();
    let skill_id = require_str(&args, "skill_id")?.to_string();
    match crate::skills::attach(pool, &caller.startup_id, &agent_id, &skill_id).await {
        Ok(()) => {
            let _ = event_tx.send(crate::protocol::ConsoleOutbound::SkillChanged {
                v: 1,
                startup_id: caller.startup_id.clone(),
                kind: "attach".to_string(),
                skill_id: skill_id.clone(),
                agent_id: Some(agent_id.clone()),
                skill: None,
            });
            Ok(json!({"ok": true}))
        }
        Err(crate::skills::SkillError::NotFound) => {
            Err(("not_found".into(), "agent or skill not found".into()))
        }
        Err(crate::skills::SkillError::CrossStartup) => Err((
            "cross_startup".into(),
            "agent or skill belongs to another startup".into(),
        )),
        Err(crate::skills::SkillError::Sql(e)) => Err(("sql".into(), e)),
        Err(_) => Err(("sql".into(), "unexpected error".into())),
    }
}

async fn handle_skill_detach(
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let agent_id = require_str(&args, "agent_id")?.to_string();
    let skill_id = require_str(&args, "skill_id")?.to_string();
    match crate::skills::detach(pool, &caller.startup_id, &agent_id, &skill_id).await {
        Ok(()) => {
            let _ = event_tx.send(crate::protocol::ConsoleOutbound::SkillChanged {
                v: 1,
                startup_id: caller.startup_id.clone(),
                kind: "detach".to_string(),
                skill_id: skill_id.clone(),
                agent_id: Some(agent_id.clone()),
                skill: None,
            });
            Ok(json!({"ok": true}))
        }
        Err(crate::skills::SkillError::NotFound) => {
            Err(("not_found".into(), "agent or skill not found".into()))
        }
        Err(crate::skills::SkillError::CrossStartup) => Err((
            "cross_startup".into(),
            "agent or skill belongs to another startup".into(),
        )),
        Err(crate::skills::SkillError::Sql(e)) => Err(("sql".into(), e)),
        Err(_) => Err(("sql".into(), "unexpected error".into())),
    }
}

async fn handle_skill_delete(
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let skill_id = require_str(&args, "skill_id")?.to_string();
    let startup_id = caller.startup_id.clone();
    match crate::skills::delete(pool, &startup_id, &skill_id).await {
        Ok(()) => {
            let _ = event_tx.send(crate::protocol::ConsoleOutbound::SkillChanged {
                v: 1,
                startup_id,
                kind: "delete".to_string(),
                skill_id: skill_id.clone(),
                agent_id: None,
                skill: None,
            });
            Ok(json!({"ok": true}))
        }
        Err(crate::skills::SkillError::NotFound) => {
            Err(("not_found".into(), format!("no skill: {skill_id}")))
        }
        Err(crate::skills::SkillError::CrossStartup) => Err((
            "cross_startup".into(),
            "skill belongs to another startup".into(),
        )),
        Err(crate::skills::SkillError::Sql(e)) => Err(("sql".into(), e)),
        Err(_) => Err(("sql".into(), "unexpected error".into())),
    }
}

/// P3 Theme C: set per-task model routing override. Manager-or-assignee may
/// override — both are reasonable: managers know the budget, assignees know
/// how heavy the task feels. Backend/model strings are not validated against
/// a catalog; that catalog evolves between releases and the cost of a typo is
/// a worker-side adapter spawn failure, not data corruption. Pass `null` or
/// omit a field to clear it; both fields default to the agent's provisioned
/// default when null.
///
/// Emits a `task_routing_changed` system_event so the operator console can
/// audit who routed what to which model. Skipped when neither field changes.
async fn handle_task_set_preference(
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let task_id = require_str(&args, "task_id")?.to_string();
    // `null` and absent are both treated as "clear", but we distinguish absent
    // (don't change) from null (clear) by reading the key presence directly.
    let backend_change = args.get("preferred_backend").map(|v| {
        if v.is_null() {
            None
        } else {
            v.as_str().map(|s| s.to_string())
        }
    });
    let model_change = args.get("preferred_model").map(|v| {
        if v.is_null() {
            None
        } else {
            v.as_str().map(|s| s.to_string())
        }
    });
    if backend_change.is_none() && model_change.is_none() {
        return Err((
            "bad_args".into(),
            "at least one of preferred_backend or preferred_model required".into(),
        ));
    }

    let task = load_task(pool, &task_id).await?;
    if task.startup_id != caller.startup_id {
        return Err((
            "cross_startup".into(),
            "task belongs to another startup".into(),
        ));
    }
    let is_manager = caller_is_manager_of_task(pool, caller, &task).await?;
    let is_assignee = task
        .assignee_agent_id
        .as_deref()
        .map(|a| a == caller.agent_id)
        .unwrap_or(false);
    if !is_manager && !is_assignee {
        return Err((
            "no_permission".into(),
            "task_set_preference: caller must be manager or assignee".into(),
        ));
    }

    // Build the UPDATE dynamically based on which fields the caller wants to
    // touch. Both branches are nullable so a clear (`null`) maps cleanly.
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| ("sql".into(), e.to_string()))?;
    if let Some(b) = &backend_change {
        sqlx::query("UPDATE tasks SET preferred_backend = ?, updated_at = unixepoch() WHERE id = ?")
            .bind(b.as_deref())
            .bind(&task_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| ("sql".into(), e.to_string()))?;
    }
    if let Some(m) = &model_change {
        sqlx::query("UPDATE tasks SET preferred_model = ?, updated_at = unixepoch() WHERE id = ?")
            .bind(m.as_deref())
            .bind(&task_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| ("sql".into(), e.to_string()))?;
    }
    tx.commit()
        .await
        .map_err(|e| ("sql".into(), e.to_string()))?;

    let _ = persist::append_audit(
        pool,
        &task_id,
        &json!({
            "actor": "agent",
            "kind": "task_set_preference",
            "agent_id": caller.agent_id,
            "preferred_backend": backend_change.clone().flatten(),
            "preferred_model": model_change.clone().flatten(),
        })
        .to_string(),
    )
    .await;

    if let Err(e) = crate::emit::emit_system_event(
        pool,
        event_tx,
        Some(&caller.startup_id),
        "task_routing_changed",
        &json!({
            "task_id": task_id,
            "set_by": caller.agent_id,
            "preferred_backend": backend_change.clone().flatten(),
            "preferred_model": model_change.clone().flatten(),
        })
        .to_string(),
        "info",
    )
    .await
    {
        tracing::warn!(component = "mcp_dispatch",
            task_id = %task_id,
            err = %e,
            "task_set_preference: system_event emit failed after UPDATE committed"
        );
    }

    Ok(json!({
        "task_id": task_id,
        "preferred_backend": backend_change.flatten(),
        "preferred_model": model_change.flatten(),
    }))
}

/// P3 carry-forward: skill_files CRUD MCP tools.
async fn handle_skill_file_upsert(
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let skill_id = require_str(&args, "skill_id")?.to_string();
    let name = require_str(&args, "name")?.to_string();
    let content = require_str(&args, "content")?.to_string();
    match crate::skills::upsert_file(pool, &caller.startup_id, &skill_id, &name, &content).await {
        Ok(id) => {
            // Reuse SkillChanged with kind="file_upsert" so any subscribed
            // console fans hear it without a new variant.
            let _ = event_tx.send(crate::protocol::ConsoleOutbound::SkillChanged {
                v: 1,
                startup_id: caller.startup_id.clone(),
                kind: "file_upsert".to_string(),
                skill_id: skill_id.clone(),
                agent_id: None,
                skill: None,
            });
            Ok(json!({"file_id": id, "skill_id": skill_id, "name": name}))
        }
        Err(crate::skills::SkillError::NotFound) => Err(("not_found".into(), "skill not found".into())),
        Err(crate::skills::SkillError::CrossStartup) => Err(("cross_startup".into(), "skill belongs to another startup".into())),
        Err(crate::skills::SkillError::BadName) => Err(("bad_name".into(), "file name contains invalid characters".into())),
        Err(crate::skills::SkillError::OversizeContent) => Err(("content_too_large".into(), "file content exceeds limit".into())),
        Err(e) => Err(("sql".into(), format!("{e:?}"))),
    }
}

async fn handle_skill_file_delete(
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let skill_id = require_str(&args, "skill_id")?.to_string();
    let name = require_str(&args, "name")?.to_string();
    match crate::skills::delete_file(pool, &caller.startup_id, &skill_id, &name).await {
        Ok(()) => {
            let _ = event_tx.send(crate::protocol::ConsoleOutbound::SkillChanged {
                v: 1,
                startup_id: caller.startup_id.clone(),
                kind: "file_delete".to_string(),
                skill_id: skill_id.clone(),
                agent_id: None,
                skill: None,
            });
            Ok(json!({"ok": true, "skill_id": skill_id, "name": name}))
        }
        Err(crate::skills::SkillError::NotFound) => Err(("not_found".into(), "skill or file not found".into())),
        Err(crate::skills::SkillError::CrossStartup) => Err(("cross_startup".into(), "skill belongs to another startup".into())),
        Err(e) => Err(("sql".into(), format!("{e:?}"))),
    }
}

/// P3 carry-forward: skill revision history. Returns up to the most recent
/// N revisions of a skill, newest first. Ownership-gated so a cross-startup
/// caller can't peek at content history.
async fn handle_skill_list_revisions(
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let skill_id = require_str(&args, "skill_id")?.to_string();
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(20)
        .min(100) as usize;
    match crate::skills::list_revisions(pool, &caller.startup_id, &skill_id).await {
        Ok(rows) => {
            let arr: Vec<serde_json::Value> = rows
                .into_iter()
                .take(limit)
                .map(|r| {
                    json!({
                        "id": r.id,
                        "skill_id": r.skill_id,
                        "rev_seq": r.rev_seq,
                        "content_md": r.content_md,
                        "created_at": r.created_at,
                        "created_by_agent_id": r.created_by_agent_id,
                        "created_by_operator_id": r.created_by_operator_id,
                    })
                })
                .collect();
            Ok(json!({"skill_id": skill_id, "revisions": arr}))
        }
        Err(crate::skills::SkillError::NotFound) => Err(("not_found".into(), "skill not found".into())),
        Err(crate::skills::SkillError::CrossStartup) => Err(("cross_startup".into(), "skill belongs to another startup".into())),
        Err(e) => Err(("sql".into(), format!("{e:?}"))),
    }
}

/// P3 carry-forward: revert a skill to a previous revision. Agent path —
/// caller must own the skill's startup (same-startup gate). Appends a NEW
/// revision row referencing the historical content so the audit log stays
/// linear.
async fn handle_skill_revert(
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let skill_id = require_str(&args, "skill_id")?.to_string();
    let rev_seq = args
        .get("rev_seq")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| ("bad_args".to_string(), "rev_seq required".to_string()))?;
    match crate::skills::revert_to_revision(
        pool,
        &caller.startup_id,
        &skill_id,
        rev_seq,
        crate::skills::Author::Agent(&caller.agent_id),
    )
    .await
    {
        Ok(content_md) => {
            let _ = event_tx.send(crate::protocol::ConsoleOutbound::SkillChanged {
                v: 1,
                startup_id: caller.startup_id.clone(),
                kind: "revert".to_string(),
                skill_id: skill_id.clone(),
                agent_id: None,
                skill: None,
            });
            Ok(json!({"skill_id": skill_id, "rev_seq": rev_seq, "len": content_md.len()}))
        }
        Err(crate::skills::SkillError::NotFound) => {
            Err(("not_found".into(), "skill or revision not found".into()))
        }
        Err(crate::skills::SkillError::CrossStartup) => Err((
            "cross_startup".into(),
            "skill belongs to another startup".into(),
        )),
        Err(e) => Err(("sql".into(), format!("{e:?}"))),
    }
}

/// P4 Theme E2: set per-task blocking dependency + deadline. Manager-or-
/// assignee may write either field. Passing `null` clears; absent leaves
/// the existing value. Editing `deadline_at` also clears
/// `deadline_notified_at` so the scheduler re-evaluates against the new
/// deadline (a postponed task shouldn't keep firing overdue events for
/// the old deadline).
async fn handle_task_set_blocking(
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let task_id = require_str(&args, "task_id")?.to_string();
    let blocked_change = args.get("blocked_on").map(|v| {
        if v.is_null() { None } else { v.as_str().map(|s| s.to_string()) }
    });
    let deadline_change = args.get("deadline_at").map(|v| {
        if v.is_null() { None } else { v.as_i64() }
    });
    if blocked_change.is_none() && deadline_change.is_none() {
        return Err((
            "bad_args".into(),
            "at least one of blocked_on or deadline_at required".into(),
        ));
    }
    let task = load_task(pool, &task_id).await?;
    if task.startup_id != caller.startup_id {
        return Err(("cross_startup".into(), "task belongs to another startup".into()));
    }
    let is_manager = caller_is_manager_of_task(pool, caller, &task).await?;
    let is_assignee = task
        .assignee_agent_id
        .as_deref()
        .map(|a| a == caller.agent_id)
        .unwrap_or(false);
    if !is_manager && !is_assignee {
        return Err((
            "no_permission".into(),
            "task_set_blocking: caller must be manager or assignee".into(),
        ));
    }
    // Reject self-blocking outright (would deadlock the scheduler).
    if let Some(Some(b)) = &blocked_change {
        if b == &task_id {
            return Err(("self_blocking".into(), "a task cannot block on itself".into()));
        }
    }
    let mut tx = pool.begin().await.map_err(|e| ("sql".into(), e.to_string()))?;
    if let Some(b) = &blocked_change {
        sqlx::query("UPDATE tasks SET blocked_on = ?, updated_at = unixepoch() WHERE id = ?")
            .bind(b.as_deref())
            .bind(&task_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| ("sql".into(), e.to_string()))?;
    }
    if let Some(d) = &deadline_change {
        // Clear the dedup stamp whenever the deadline shifts so a new
        // overdue event can fire against the new boundary.
        sqlx::query(
            "UPDATE tasks SET deadline_at = ?, deadline_notified_at = NULL, \
                              updated_at = unixepoch() WHERE id = ?",
        )
        .bind(*d)
        .bind(&task_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| ("sql".into(), e.to_string()))?;
    }
    tx.commit().await.map_err(|e| ("sql".into(), e.to_string()))?;

    let _ = persist::append_audit(
        pool,
        &task_id,
        &json!({
            "actor": "agent",
            "kind": "task_set_blocking",
            "agent_id": caller.agent_id,
            "blocked_on": blocked_change.clone().flatten(),
            "deadline_at": deadline_change.flatten(),
        })
        .to_string(),
    )
    .await;

    // Surface the change so the operator console can refresh the row.
    if let Err(e) = crate::emit::emit_system_event(
        pool,
        event_tx,
        Some(&caller.startup_id),
        "task_blocking_changed",
        &json!({
            "task_id": task_id,
            "blocked_on": blocked_change.clone().flatten(),
            "deadline_at": deadline_change.flatten(),
        })
        .to_string(),
        "info",
    )
    .await
    {
        tracing::warn!(component = "mcp_dispatch",
            task_id = %task_id, err = %e,
            "task_set_blocking: system_event emit failed after UPDATE"
        );
    }

    Ok(json!({
        "task_id": task_id,
        "blocked_on": blocked_change.flatten(),
        "deadline_at": deadline_change.flatten(),
    }))
}
