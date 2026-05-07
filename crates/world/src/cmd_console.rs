//! Operator-side command dispatcher. Called from loop_::spawn's HandleConsoleMsg arm.
//! Parses ConsoleInbound, applies the world mutation + SQLite write, returns a JSON reply.

use crate::persist;
use crate::protocol::ConsoleInbound;
use crate::state::{AvatarView, WorldView};
use crate::task_sm::{next, Actor, TaskStatus, Transition};
use serde_json::json;
use sqlx::SqlitePool;

const OPERATOR_AVATAR_ID: &str = "__operator__";

/// Dispatch a single console message. Mutates `world` and writes to `pool`.
/// Returns a JSON value that becomes the WS reply.
pub async fn dispatch(
    world: &mut WorldView,
    pool: &SqlitePool,
    msg: serde_json::Value,
) -> serde_json::Value {
    let inbound: ConsoleInbound = match serde_json::from_value(msg.clone()) {
        Ok(v) => v,
        Err(e) => return json!({"type":"error","reason":"parse","detail":e.to_string()}),
    };

    match inbound {
        ConsoleInbound::Hello { .. } => {
            // Auth was already validated in http::handle_console; subsequent hello is a no-op echo.
            json!({"type":"ok","kind":"hello"})
        }
        ConsoleInbound::OperatorMove { target_x, target_y, .. } => {
            // The operator avatar must be possessing a town for move to be meaningful;
            // if absent, this is a no-op with an error.
            if let Some(avatar) = world.avatars.get_mut(OPERATOR_AVATAR_ID) {
                avatar.target_pos = Some((target_x, target_y));
                json!({"type":"ok","kind":"operator_move"})
            } else {
                json!({"type":"error","reason":"not_possessing"})
            }
        }
        ConsoleInbound::OperatorPossess { startup_id, .. } => {
            // Spawn operator avatar in the lobby of `startup_id`'s town.
            // For Phase 0 the town id is fixed at "town_default" and lobby is at (20, 5).
            let avatar = AvatarView {
                agent_id: OPERATOR_AVATAR_ID.to_string(),
                startup_id: startup_id.clone(),
                role: "operator".to_string(),
                backend: "operator".to_string(),
                current_pos: (20, 5),
                target_pos: None,
                room_id: "lobby".to_string(),
                status: "idle".to_string(),
            };
            world.avatars.insert(OPERATOR_AVATAR_ID.to_string(), avatar);
            json!({"type":"ok","kind":"operator_possess","startup_id":startup_id})
        }
        ConsoleInbound::OperatorUnpossess { .. } => {
            world.avatars.remove(OPERATOR_AVATAR_ID);
            json!({"type":"ok","kind":"operator_unpossess"})
        }
        ConsoleInbound::OperatorDirective { to_agent_id, body, .. } => {
            // Insert a directive message in SQLite. M1.13+ will route it to the worker.
            let id = uuid::Uuid::new_v4().to_string();
            let r = sqlx::query(
                "INSERT INTO messages (id, startup_id, room_id, author_id, body, kind, ts) \
                 VALUES (?, (SELECT startup_id FROM agents WHERE id = ?), NULL, 'operator', ?, 'directive', unixepoch())"
            )
            .bind(&id)
            .bind(&to_agent_id)
            .bind(&body)
            .execute(pool)
            .await;
            match r {
                Ok(_) => json!({"type":"ok","kind":"operator_directive","message_id":id}),
                Err(e) => json!({"type":"error","reason":"sql","detail":e.to_string()}),
            }
        }
        ConsoleInbound::OperatorAcceptProposal { task_id, assignee_agent_id, required_room, .. } => {
            // Inlined (deviation from plan): the generic `apply_task_transition` closure
            // approach has tricky sqlx::Query lifetime constraints; inlining the
            // accept_proposal flow is simpler and keeps the audit + state-machine call
            // + UPDATE in a single readable block.
            let current_row: Result<(String,), _> =
                sqlx::query_as("SELECT status FROM tasks WHERE id = ?")
                    .bind(&task_id)
                    .fetch_one(pool)
                    .await;
            let current = match current_row {
                Ok((s,)) => match parse_status(&s) {
                    Some(s) => s,
                    None => return json!({"type":"error","reason":"unknown_status","status":s}),
                },
                Err(e) => return json!({"type":"error","reason":"sql","detail":e.to_string()}),
            };
            if next(current, &Transition::AcceptProposal { caller: Actor::Operator }).is_err() {
                return json!({"type":"error","reason":"illegal_transition"});
            }
            let r = sqlx::query(
                "UPDATE tasks SET status = 'queued', assignee_agent_id = ?, required_room = ?, updated_at = unixepoch() WHERE id = ?"
            )
            .bind(&assignee_agent_id)
            .bind(&required_room)
            .bind(&task_id)
            .execute(pool)
            .await;
            let _ = persist::append_audit(
                pool,
                &task_id,
                &json!({"actor":"operator","kind":"operator_accept_proposal"}).to_string(),
            )
            .await;
            match r {
                Ok(_) => json!({"type":"ok","kind":"operator_accept_proposal","task_id":task_id}),
                Err(e) => json!({"type":"error","reason":"sql","detail":e.to_string()}),
            }
        }
        ConsoleInbound::OperatorRejectProposal { task_id, reason, .. } => {
            // Append audit before transitioning.
            let _ = persist::append_audit(
                pool,
                &task_id,
                &json!({
                    "actor":"operator","kind":"reject_proposal","reason":reason
                })
                .to_string(),
            )
            .await;
            apply_status_only_transition(
                pool,
                &task_id,
                &Transition::RejectProposal { caller: Actor::Operator },
                "operator_reject_proposal",
            )
            .await
        }
        ConsoleInbound::OperatorForceAccept { task_id, .. } => {
            let result = apply_status_only_transition(
                pool,
                &task_id,
                &Transition::OperatorForceAccept,
                "operator_force_accept",
            )
            .await;
            // Only write the audit on a successful transition.
            if result["type"] == "ok" {
                let _ = persist::append_audit(
                    pool,
                    &task_id,
                    &json!({"actor":"operator","kind":"force_accept"}).to_string(),
                )
                .await;
            }
            result
        }
        ConsoleInbound::OperatorForceFail { task_id, note, .. } => {
            let result = apply_status_only_transition(
                pool,
                &task_id,
                &Transition::OperatorForceFail,
                "operator_force_fail",
            )
            .await;
            if result["type"] == "ok" {
                let _ = persist::append_audit(
                    pool,
                    &task_id,
                    &json!({"actor":"operator","kind":"force_fail","note":note}).to_string(),
                )
                .await;
            }
            result
        }
        ConsoleInbound::OperatorRecheckBackends => {
            // The actual probe is fired by the SIGHUP handler / 5-min refresh in main.rs;
            // M1.4 wired the loop's Cmd::BackendCatalogUpdated. Operator-triggered recheck
            // is a future hook; for now reply OK and let the operator fall back to the
            // POST /api/backend-catalog/recheck HTTP endpoint.
            json!({"type":"ok","kind":"operator_recheck_backends","note":"use POST /api/backend-catalog/recheck"})
        }
    }
}

/// Generic helper: read current task status, compute new status via task_sm, run UPDATE.
async fn apply_status_only_transition(
    pool: &SqlitePool,
    task_id: &str,
    transition: &Transition,
    audit_kind: &str,
) -> serde_json::Value {
    let current_row: Result<(String,), _> = sqlx::query_as("SELECT status FROM tasks WHERE id = ?")
        .bind(task_id)
        .fetch_one(pool)
        .await;
    let current = match current_row {
        Ok((s,)) => match parse_status(&s) {
            Some(s) => s,
            None => return json!({"type":"error","reason":"unknown_status","status":s}),
        },
        Err(e) => return json!({"type":"error","reason":"sql","detail":e.to_string()}),
    };
    let new_status = match next(current, transition) {
        Ok(s) => s,
        Err(reason) => return json!({"type":"error","reason":"illegal_transition","detail":reason}),
    };
    let new_str = status_to_str(new_status);
    let r = sqlx::query("UPDATE tasks SET status = ?, updated_at = unixepoch() WHERE id = ?")
        .bind(new_str)
        .bind(task_id)
        .execute(pool)
        .await;
    match r {
        Ok(_) => json!({"type":"ok","kind":audit_kind,"task_id":task_id,"new_status":new_str}),
        Err(e) => json!({"type":"error","reason":"sql","detail":e.to_string()}),
    }
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
