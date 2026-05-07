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

type HandlerResult = Result<Value, (String, String)>;

pub async fn dispatch(
    world: &mut WorldView,
    paths: &mut PathStore,
    layout: &TownLayout,
    graph: &RoomGraph,
    out_bus: &HashMap<String, mpsc::Sender<Value>>,
    pool: &SqlitePool,
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

    let result: HandlerResult = match tool.as_str() {
        "move_intent" => handle_move_intent(world, paths, layout, graph, &caller, args).await,
        "speak" => handle_speak(world, out_bus, pool, &caller, args).await,
        "task_done" => handle_task_done(out_bus, pool, &caller, args).await,
        "task_failed" => handle_task_failed(pool, &caller, args).await,
        "subtask_create" => handle_subtask_create(out_bus, pool, &caller, args).await,
        "task_accept" => handle_task_accept(out_bus, pool, &caller, args).await,
        "task_request_changes" => handle_task_request_changes(out_bus, pool, &caller, args).await,
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
        _ => Err((
            "unknown_tool".into(),
            format!("no handler for tool: {}", tool),
        )),
    };

    match result {
        Ok(v) => json!({"type":"mcp_reply","v":1,"corr_id":corr_id,"result":v}),
        Err((code, message)) => mcp_err(&corr_id, &code, &message),
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

// ── handlers ────────────────────────────────────────────────────────────────

async fn handle_move_intent(
    world: &mut WorldView,
    paths: &mut PathStore,
    layout: &TownLayout,
    graph: &RoomGraph,
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
        StartMoveResult::PermissionDenied => Err((
            "no_permission".into(),
            "cannot enter target room".into(),
        )),
        StartMoveResult::NoSuchAgent => Err(("unknown_agent".into(), "agent not found".into())),
    }
}

async fn handle_speak(
    world: &WorldView,
    out_bus: &HashMap<String, mpsc::Sender<Value>>,
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let body = require_str(&args, "body")?.to_string();
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
        // Fan to every same-startup peer in the same room. Spec §5.4:
        // chat is room-scoped, not org-scoped. Cross-startup peers in a
        // common room will hear nothing in Phase 0 because we keep the
        // startup_id filter; that simplification is intentional.
        for (peer_id, peer) in &world.avatars {
            if peer_id == &caller.agent_id {
                continue;
            }
            if peer.room_id == caller.room_id && peer.startup_id == caller.startup_id {
                if let Some(tx) = out_bus.get(peer_id) {
                    let _ = tx.try_send(json!({
                        "type":"chat_received","v":1,
                        "from_agent_id":caller.agent_id,
                        "body":body,
                        "room_id":caller.room_id
                    }));
                }
            }
        }
    } else if let Some(rid) = to_agent_id.as_deref() {
        if let Some(tx) = out_bus.get(rid) {
            let _ = tx.try_send(json!({
                "type":"directive","v":1,
                "from_agent_id":caller.agent_id,
                "body":body
            }));
        }
    }

    Ok(json!({"message_id": id}))
}

async fn handle_task_done(
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

    // Re-validate artifact path against the startup's workspace root. This is
    // the world-side defense-in-depth gate; the worker also enforces in
    // `sandbox::resolve` before exposing the path back, but Phase 0 spec is
    // explicit that the world must re-check (see §6.3).
    let workspace_root = std::path::PathBuf::from(format!("workspaces/{}", caller.startup_id));
    sandbox::resolve(&workspace_root, &artifact_path)
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
                tracing::warn!(
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
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let task_id = require_str(&args, "task_id")?.to_string();
    let feedback = require_str(&args, "feedback")?.to_string();
    let task = load_task(pool, &task_id).await?;
    if task.startup_id != caller.startup_id {
        return Err(("cross_startup".into(), "task belongs to another startup".into()));
    }
    if !caller_is_manager_of_task(pool, caller, &task).await? {
        return Err((
            "no_permission".into(),
            "task_request_changes is manager-only".into(),
        ));
    }
    let new_status = next(task.status, &Transition::RequestChanges)
        .map_err(|r| ("illegal_transition".to_string(), r.to_string()))?;
    sqlx::query(
        "UPDATE tasks SET status = ?, review_round = review_round + 1, updated_at = unixepoch() WHERE id = ?",
    )
    .bind(status_to_str(new_status))
    .bind(&task_id)
    .execute(pool)
    .await
    .map_err(|e| ("sql".to_string(), e.to_string()))?;
    let _ = persist::append_audit(
        pool,
        &task_id,
        &json!({"actor":"manager","kind":"task_request_changes","agent_id":caller.agent_id})
            .to_string(),
    )
    .await;

    // Surface feedback to the assignee as a `directive` event so it's picked
    // up on their next CLI session boot (spec §7 Level 2).
    if let Some(assignee) = task.assignee_agent_id.as_deref() {
        if let Some(tx) = out_bus.get(assignee) {
            let _ = tx.try_send(json!({
                "type":"directive","v":1,
                "from_agent_id": caller.agent_id,
                "body": feedback,
                "in_response_to_task": task_id,
            }));
        }
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
