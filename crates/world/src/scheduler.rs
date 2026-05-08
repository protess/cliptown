//! Task scheduler. Runs once per `Cmd::Tick` (1Hz).
//!
//! Picks queued tasks whose assignee is idle, transitions them to `in_progress`,
//! and dispatches `task_assigned` to the assignee's worker via `out_bus`. If the
//! task has a `required_room` and the agent isn't in it, the scheduler kicks
//! off a move toward that room (via `move_sys::start_move`) and leaves the
//! task in `queued` state — a subsequent tick (after arrival) will dispatch.
//!
//! Phase-0 design notes:
//! - Stateless: re-queries SQL each tick; no in-memory dedup. The fact that an
//!   agent's status flips to `working` after dispatch keeps re-dispatch from
//!   firing twice in the same lifecycle.
//! - Out-of-band: this runs after `move_sys::step_all` in the tick handler,
//!   so a `move_complete` from this tick is already reflected in `room_id`
//!   before the scheduler checks `required_room`.

use crate::move_sys::{self, PathStore};
use crate::path::RoomGraph;
use crate::protocol::WorkerOutbound;
use crate::seed::TownLayout;
use crate::state::WorldView;
use serde_json::json;
use sqlx::SqlitePool;
use std::collections::HashMap;
use tokio::sync::mpsc;

#[derive(Debug, sqlx::FromRow)]
struct QueuedTask {
    id: String,
    title: String,
    description: String,
    assignee_agent_id: String,
    required_room: Option<String>,
    parent_id: Option<String>,
}

/// Run one scheduler tick. Returns the number of tasks dispatched
/// (transitioned to `in_progress` and pushed to the worker out_bus).
pub async fn tick(
    world: &mut WorldView,
    paths: &mut PathStore,
    layout: &TownLayout,
    graph: &RoomGraph,
    out_bus: &HashMap<String, mpsc::Sender<serde_json::Value>>,
    pool: &SqlitePool,
) -> usize {
    let queued: Vec<QueuedTask> = match sqlx::query_as(
        "SELECT id, title, description, assignee_agent_id, required_room, parent_id FROM tasks \
         WHERE status = 'queued' AND assignee_agent_id IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(component = "scheduler", error = %e, "scheduler: query failed");
            return 0;
        }
    };

    let mut dispatched = 0usize;
    for task in queued {
        let agent_id = task.assignee_agent_id.clone();

        // Snapshot the avatar's status/room before any mutation.
        let (avatar_status, avatar_room) = match world.avatars.get(&agent_id) {
            Some(a) => (a.status.clone(), a.room_id.clone()),
            None => continue, // agent not yet connected; retry next tick.
        };

        if avatar_status != "idle" {
            continue; // agent is busy; wait until it finishes its current work.
        }

        // If a required_room is set and the agent is not in it, kick off a
        // move toward that room. Skip dispatch this tick; a future tick (post
        // arrival, where status returns to idle) will retry.
        if let Some(room) = task.required_room.as_deref() {
            if avatar_room != room {
                // If a path is already in flight for this agent, let
                // `step_all` finish it; don't recompute A* every tick.
                // The agent's status stays "idle" during transit, so without
                // this short-circuit we'd re-enter this branch each tick.
                if paths.contains_key(&agent_id) {
                    continue;
                }
                if let Some((cx, cy)) = pick_room_center(layout, room) {
                    match move_sys::start_move(
                        world, paths, layout, graph, &agent_id, room, cx, cy,
                    ) {
                        move_sys::StartMoveResult::Ok => {}
                        other => {
                            tracing::warn!(component = "scheduler",
                                task_id = %task.id,
                                agent_id = %agent_id,
                                room = %room,
                                result = ?other,
                                "scheduler: cannot route to required_room"
                            );
                        }
                    }
                }
                continue;
            }
        }

        // Budget gate (M1.15): refuse new dispatches once spend ≥ 95% of cap.
        // The 100% threshold issues a `Pause` to all of the startup's workers
        // (see `budget::pause_startup`), but Phase 0 doesn't ack a Pause back
        // to the world; the gate here is what keeps queued tasks from being
        // handed to a worker that's already supposed to be paused.
        let budget: Result<(f64, f64), _> = sqlx::query_as(
            "SELECT budget_spent_usd, budget_cap_usd FROM startups \
             WHERE id = (SELECT startup_id FROM agents WHERE id = ?)",
        )
        .bind(&agent_id)
        .fetch_one(pool)
        .await;
        if let Ok((spent, cap)) = budget {
            if cap > 0.0 && spent / cap >= 0.95 {
                continue;
            }
        }

        // Codex round-5 P1#1: gate the transition on a live delivery target.
        // Without this, a worker that's still connecting (or that just
        // disconnected) ends up with a task wedged in `in_progress` in SQL
        // while no `task_assigned` ever arrives — the scheduler only re-queries
        // `queued` tasks, so it never re-dispatches. Skip this iteration; the
        // task stays `queued` and the next tick will retry once the worker
        // registers.
        if !out_bus.contains_key(&agent_id) {
            continue;
        }

        // Agent is idle, in the right room, and the worker is registered.
        // Transition the task and notify the worker.
        let r = sqlx::query(
            "UPDATE tasks SET status = 'in_progress', updated_at = unixepoch() WHERE id = ?",
        )
        .bind(&task.id)
        .execute(pool)
        .await;
        if let Err(e) = r {
            tracing::warn!(component = "scheduler", task_id = %task.id, error = %e, "scheduler: status update failed");
            continue;
        }

        // Mark the avatar busy in-memory; the worker will report further
        // status changes (e.g. back to idle on completion).
        if let Some(a) = world.avatars.get_mut(&agent_id) {
            a.status = "working".to_string();
        }

        // Record the dispatch in the task's audit_trail. Spec §3 lists
        // `task_assigned` as an audit event used by the /console event feed
        // and post-mortem replay.
        let _ = crate::persist::append_audit(
            pool,
            &task.id,
            &json!({
                "actor": "scheduler",
                "kind": "task_assigned",
                "agent_id": agent_id,
                "required_room": task.required_room,
            })
            .to_string(),
        )
        .await;

        let payload = WorkerOutbound::TaskAssigned {
            v: 1,
            task_id: task.id.clone(),
            title: task.title.clone(),
            description: task.description.clone(),
            required_room: task.required_room.clone(),
            parent_id: task.parent_id.clone(),
        };
        let payload_json = serde_json::to_value(&payload).unwrap_or_else(|_| json!({}));
        // The contains_key check above is racy w.r.t. a worker disconnect
        // between then and now; treat a missing entry the same as a closed
        // channel and roll back.
        let send_result = match out_bus.get(&agent_id) {
            Some(tx) => tx.try_send(payload_json),
            None => Err(tokio::sync::mpsc::error::TrySendError::Closed(json!({}))),
        };
        if let Err(err) = send_result {
            // Channel full or closed — roll back the SQL transition and the
            // in-memory avatar status so the next tick re-attempts dispatch.
            let kind = match err {
                tokio::sync::mpsc::error::TrySendError::Full(_) => "full",
                tokio::sync::mpsc::error::TrySendError::Closed(_) => "closed",
            };
            let _ = sqlx::query(
                "UPDATE tasks SET status = 'queued', updated_at = unixepoch() WHERE id = ?",
            )
            .bind(&task.id)
            .execute(pool)
            .await;
            if let Some(a) = world.avatars.get_mut(&agent_id) {
                a.status = "idle".to_string();
            }
            tracing::warn!(
                component = "scheduler",
                agent_id = %agent_id,
                task_id = %task.id,
                reason = kind,
                "out_bus delivery failed, rolled back to queued"
            );
            continue;
        }
        dispatched += 1;
    }
    dispatched
}

/// Returns the center tile of `room_id`'s bounds, or `None` if the room
/// doesn't exist in the layout. Used as a movement target when a task has
/// a `required_room` and the agent must move there.
fn pick_room_center(layout: &TownLayout, room_id: &str) -> Option<(i32, i32)> {
    layout.room(room_id).map(|r| {
        let cx = r.bounds.0 + r.bounds.2 / 2;
        let cy = r.bounds.1 + r.bounds.3 / 2;
        (cx, cy)
    })
}
