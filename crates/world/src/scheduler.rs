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

use crate::agent_supervisor::{
    per_task_workers_enabled, AgentSupervisor, SpawnConfig, TaskSpawn,
};
use crate::move_sys::{self, PathStore};
use crate::path::RoomGraph;
use crate::protocol::WorkerOutbound;
use crate::seed::TownLayout;
use crate::state::WorldView;
use serde_json::json;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, sqlx::FromRow)]
struct QueuedTask {
    id: String,
    title: String,
    description: String,
    assignee_agent_id: String,
    required_room: Option<String>,
    parent_id: Option<String>,
    /// P3 Theme C: per-task routing override. NULL falls back to whatever
    /// the worker decides (typically its provisioned default).
    preferred_backend: Option<String>,
    preferred_model: Option<String>,
    /// P3 Theme C follow-up (Option B): joined from `agents.backend`. Used as
    /// the default backend when `preferred_backend` is NULL and the scheduler
    /// is in per-task mode (the field is also forwarded to `--backend` so the
    /// worker has a fallback when no preferred override is set).
    startup_id: String,
    agent_backend: String,
    workspace_path: String,
    /// P4 Theme E2: blocking dependency. When non-NULL, dispatch is held
    /// until the referenced task reaches a terminal state.
    blocked_on: Option<String>,
}

/// Run one scheduler tick. Returns the number of tasks dispatched
/// (transitioned to `in_progress` and either pushed to the worker out_bus
/// in legacy mode or handed to the supervisor to spawn a one-shot worker
/// process in per-task mode).
///
/// `supervisor` is honored only when `CLIPTOWN_PER_TASK_WORKERS=1` AND a
/// handle is supplied (loop_::spawn passes one in production; tests pass
/// `None` to keep the legacy out_bus path).
/// `event_tx` (added P4 Theme E2) is the broadcast channel for
/// `task_unblocked` / `task_overdue` system_events emitted from this tick.
pub async fn tick(
    world: &mut WorldView,
    paths: &mut PathStore,
    layout: &TownLayout,
    graph: &RoomGraph,
    out_bus: &HashMap<String, mpsc::Sender<serde_json::Value>>,
    pool: &SqlitePool,
    supervisor: Option<&Arc<AgentSupervisor>>,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
) -> usize {
    // P3 Theme D follow-up: tick-level tracing event so post-hoc analysis can
    // bucket dispatch latency by tick. Same event-pair pattern as
    // mcp_dispatch — no Span guard because the tick handler awaits SQL.
    let tick_start = std::time::Instant::now();
    let tick_seq = world.tick_seq;
    // P4 Theme E2: surface `blocked_on` so the dispatch loop can hold tasks
    // whose dependency hasn't finished. Deadline scan runs separately
    // below — it touches every non-terminal task with a deadline, not just
    // queued ones, so it doesn't piggyback on this query.
    let queued: Vec<QueuedTask> = match sqlx::query_as(
        "SELECT t.id, t.title, t.description, t.assignee_agent_id, t.required_room, \
                t.parent_id, t.preferred_backend, t.preferred_model, \
                t.startup_id, a.backend AS agent_backend, s.workspace_path, \
                t.blocked_on \
         FROM tasks t \
         JOIN agents a ON a.id = t.assignee_agent_id \
         JOIN startups s ON s.id = t.startup_id \
         WHERE t.status = 'queued' AND t.assignee_agent_id IS NOT NULL",
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

        // P4 Theme E2: blocking-gate check. When `blocked_on` is set, only
        // dispatch when the dependency has reached a terminal state. Emits
        // `task_unblocked` once on the transitioning tick so the operator
        // console can refresh; uses the SQL row's `blocked_on = NULL` write
        // as the dedup signal (we clear it before emit so re-emit can't
        // fire a second time).
        if let Some(blocker_id) = &task.blocked_on {
            let blocker_status: Result<Option<(String,)>, _> =
                sqlx::query_as("SELECT status FROM tasks WHERE id = ?")
                    .bind(blocker_id)
                    .fetch_optional(pool)
                    .await;
            let is_terminal = match blocker_status {
                Ok(Some((s,))) => matches!(s.as_str(), "done" | "failed" | "escalated"),
                Ok(None) => true, // blocker row deleted; treat as resolved.
                Err(e) => {
                    tracing::warn!(component = "scheduler",
                        task_id = %task.id, blocker_id = %blocker_id, error = %e,
                        "blocker status query failed"
                    );
                    continue;
                }
            };
            if !is_terminal {
                continue; // hold the task; next tick re-checks.
            }
            // Dependency cleared. Clear the column + emit unblocked event
            // before proceeding to dispatch.
            let _ = sqlx::query("UPDATE tasks SET blocked_on = NULL WHERE id = ?")
                .bind(&task.id)
                .execute(pool)
                .await;
            let _ = crate::emit::emit_system_event(
                pool,
                event_tx,
                Some(&task.startup_id),
                "task_unblocked",
                &json!({"task_id": task.id, "blocker_id": blocker_id}).to_string(),
                "info",
            )
            .await;
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

        // Codex round-5 P1#1 / Theme C Option B: liveness gate, polarity
        // depends on the mode.
        //
        // Legacy (long-running daemon): a worker that's still connecting (or
        // that just disconnected) ends up with a task wedged in `in_progress`
        // in SQL while no `task_assigned` ever arrives — the scheduler only
        // re-queries `queued` tasks, so it never re-dispatches. We require
        // an `out_bus` entry before flipping state.
        //
        // Per-task (`CLIPTOWN_PER_TASK_WORKERS=1`): inverse — an out_bus
        // entry means a previously-spawned worker is still mid-task for this
        // agent. Don't double-spawn. Avatar.status == "idle" + out_bus empty
        // = safe to spawn a fresh worker.
        let per_task_mode = per_task_workers_enabled() && supervisor.is_some();
        if per_task_mode {
            if out_bus.contains_key(&agent_id) {
                continue;
            }
        } else if !out_bus.contains_key(&agent_id) {
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

        let dispatch_failed_reason: Option<&'static str> = if per_task_mode {
            // Per-task: hand off to the supervisor. The supervisor returns
            // once the spawn syscall has succeeded; the worker connects WS,
            // spawns its adapter, runs to completion, and exits cleanly. The
            // supervisor's existing watch_loop returns on clean exit so no
            // respawn fires.
            let sup = supervisor.expect("checked above");
            let secret = std::env::var(format!("CLIPTOWN_AGENT_SECRET_{}", agent_id))
                .unwrap_or_else(|_| "dev-secret".to_string());
            let world_url = std::env::var("CLIPTOWN_WORLD_WS_URL")
                .unwrap_or_else(|_| "ws://127.0.0.1:8080/ws/worker".to_string());
            let prompt = format!(
                "You are agent {agent} in startup {sid}. Implement task '{title}':\n\n\
                 {desc}\n\nWhen complete, call the `task_done` MCP tool with \
                 artifact_path \"workspaces/{sid}/artifacts/{tid}.md\". Save your \
                 output to that path first.",
                agent = agent_id,
                sid = task.startup_id,
                title = task.title,
                desc = task.description,
                tid = task.id,
            );
            let cfg = SpawnConfig {
                agent_id: agent_id.clone(),
                startup_id: task.startup_id.clone(),
                world_url,
                secret,
                workspace: task.workspace_path.clone(),
                backend: task.agent_backend.clone(),
                task: Some(TaskSpawn {
                    task_id: task.id.clone(),
                    prompt,
                    preferred_backend: task.preferred_backend.clone(),
                    preferred_model: task.preferred_model.clone(),
                }),
            };
            match sup.spawn_agent(cfg).await {
                Ok(()) => None,
                Err(e) => {
                    tracing::warn!(component = "scheduler",
                        agent_id = %agent_id, task_id = %task.id, error = %e,
                        "supervisor spawn_for_task failed"
                    );
                    Some("spawn_failed")
                }
            }
        } else {
            let payload = WorkerOutbound::TaskAssigned {
                v: 1,
                task_id: task.id.clone(),
                title: task.title.clone(),
                description: task.description.clone(),
                required_room: task.required_room.clone(),
                parent_id: task.parent_id.clone(),
                preferred_backend: task.preferred_backend.clone(),
                preferred_model: task.preferred_model.clone(),
            };
            let payload_json = serde_json::to_value(&payload).unwrap_or_else(|_| json!({}));
            // The contains_key check above is racy w.r.t. a worker disconnect
            // between then and now; treat a missing entry the same as a closed
            // channel and roll back.
            let send_result = match out_bus.get(&agent_id) {
                Some(tx) => tx.try_send(payload_json),
                None => Err(tokio::sync::mpsc::error::TrySendError::Closed(json!({}))),
            };
            match send_result {
                Ok(()) => None,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Some("full"),
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => Some("closed"),
            }
        };

        if let Some(reason) = dispatch_failed_reason {
            let _ = sqlx::query(
                "UPDATE tasks SET status = 'queued', updated_at = unixepoch() WHERE id = ?",
            )
            .bind(&task.id)
            .execute(pool)
            .await;
            if let Some(a) = world.avatars.get_mut(&agent_id) {
                a.status = "idle".to_string();
            }
            tracing::warn!(component = "scheduler",
                agent_id = %agent_id,
                task_id = %task.id,
                reason = reason,
                "dispatch failed, rolled back to queued"
            );
            continue;
        }
        dispatched += 1;
    }

    // P4 Theme E2: deadline scan. Per tick, find non-terminal tasks whose
    // `deadline_at` has passed and we haven't notified yet (
    // `deadline_notified_at IS NULL OR < deadline_at`), emit one
    // `task_overdue` system_event each, and stamp `deadline_notified_at`
    // to dedup the next tick. A deadline change (UPDATE deadline_at)
    // should clear `deadline_notified_at`; the MCP tool below handles
    // that.
    let now = chrono::Utc::now().timestamp();
    let overdue: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT id, startup_id, deadline_at FROM tasks \
         WHERE deadline_at IS NOT NULL AND deadline_at < ? \
           AND status NOT IN ('done', 'failed', 'escalated') \
           AND (deadline_notified_at IS NULL OR deadline_notified_at < deadline_at)",
    )
    .bind(now)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    for (task_id, startup_id, deadline_at) in overdue {
        let _ = crate::emit::emit_system_event(
            pool,
            event_tx,
            Some(&startup_id),
            "task_overdue",
            &json!({
                "task_id": task_id,
                "deadline_at": deadline_at,
                "overdue_by_secs": now - deadline_at,
            })
            .to_string(),
            "warn",
        )
        .await;
        let _ = sqlx::query("UPDATE tasks SET deadline_notified_at = ? WHERE id = ?")
            .bind(now)
            .bind(&task_id)
            .execute(pool)
            .await;
    }

    // P4 Theme E3: opt-in auto work-stealing pass. For each startup with
    // `auto_steal_enabled = 1`, find queued tasks whose assignee is busy and
    // whose `updated_at` is older than the per-startup threshold, then
    // reassign to an idle same-role peer in the same startup. Manual
    // `task_steal` handles ad-hoc claims; this pass rebalances the chronic
    // case where one engineer's queue stays warm while a peer sits idle.
    auto_steal_pass(world, pool, event_tx).await;

    // P6 Theme C: opt-in auto-recovery pass. For each startup with
    // `auto_recovery_enabled = 1`, find tasks in `changes_requested`
    // whose `review_round >= auto_recovery_max_attempts` and reassign
    // to an idle same-role peer (resetting status to `queued`). If no
    // peer is available, the task stays where it is and the next
    // round eventually escalates via the existing flow.
    auto_recovery_pass(world, pool, event_tx).await;

    if dispatched > 0 || tick_start.elapsed().as_millis() > 5 {
        // Only log non-trivial ticks. Quiet ticks (0 dispatches, <5ms) are
        // skipped so /metrics' tick_seq counter is the cheap path; this
        // event is for "something happened" replay.
        tracing::debug!(
            component = "scheduler",
            event = "tick_complete",
            tick_seq,
            dispatched,
            elapsed_us = tick_start.elapsed().as_micros() as u64,
        );
    }
    dispatched
}

/// P4 Theme E3: per-tick auto-steal pass. Walks every startup with
/// `auto_steal_enabled = 1`, collects queued tasks that have sat untouched
/// past the per-startup threshold AND whose current assignee is busy,
/// and reassigns them to an idle same-role peer when one exists. The
/// reassignment uses a same-row WHERE clause (`assignee = ?, status =
/// 'queued'`) so a concurrent manual steal between SELECT and UPDATE
/// loses the race cleanly. Bumping `updated_at` after the swap prevents
/// the same row from re-firing on the next tick.
async fn auto_steal_pass(
    world: &WorldView,
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
) {
    let now = chrono::Utc::now().timestamp();
    let startups: Vec<(String, i64)> = sqlx::query_as(
        "SELECT id, auto_steal_after_secs FROM startups WHERE auto_steal_enabled = 1",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    for (startup_id, after_secs) in startups {
        let cutoff = now - after_secs.max(1);
        let stale: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, assignee_agent_id FROM tasks \
             WHERE startup_id = ? AND status = 'queued' \
               AND assignee_agent_id IS NOT NULL \
               AND updated_at < ?",
        )
        .bind(&startup_id)
        .bind(cutoff)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        for (task_id, current_assignee) in stale {
            // Current assignee must NOT be idle — if they're idle the
            // normal dispatch loop already picked it up (or will next
            // tick). Stealing from an idle peer would just churn.
            let assignee_status = world
                .avatars
                .get(&current_assignee)
                .map(|a| a.status.clone());
            if assignee_status.as_deref() == Some("idle") {
                continue;
            }
            // Find an idle same-role peer in the same startup. Pick the
            // first match — Phase 0 doesn't load-balance among multiple
            // idle peers; that's a fairness concern for later.
            let assignee_role: Option<String> =
                sqlx::query_scalar("SELECT role FROM agents WHERE id = ?")
                    .bind(&current_assignee)
                    .fetch_optional(pool)
                    .await
                    .unwrap_or(None);
            let Some(role) = assignee_role else { continue };
            let peer_id = world.avatars.iter().find_map(|(id, a)| {
                if a.startup_id == startup_id
                    && a.role == role
                    && a.status == "idle"
                    && *id != current_assignee
                {
                    Some(id.clone())
                } else {
                    None
                }
            });
            let Some(peer_id) = peer_id else { continue };

            let res = sqlx::query(
                "UPDATE tasks SET assignee_agent_id = ?, updated_at = unixepoch() \
                 WHERE id = ? AND status = 'queued' AND assignee_agent_id = ?",
            )
            .bind(&peer_id)
            .bind(&task_id)
            .bind(&current_assignee)
            .execute(pool)
            .await;
            let rows = match res {
                Ok(r) => r.rows_affected(),
                Err(e) => {
                    tracing::warn!(component = "scheduler",
                        task_id = %task_id, error = %e,
                        "auto-steal UPDATE failed"
                    );
                    continue;
                }
            };
            if rows == 0 {
                continue;
            }
            let _ = crate::persist::append_audit(
                pool,
                &task_id,
                &json!({
                    "actor": "stealer",
                    "kind": "task_steal",
                    "agent_id": peer_id,
                    "previous_assignee": current_assignee,
                    "mode": "auto",
                })
                .to_string(),
            )
            .await;
            let _ = crate::emit::emit_system_event(
                pool,
                event_tx,
                Some(&startup_id),
                "task_stolen",
                &json!({
                    "task_id": task_id,
                    "new_assignee": peer_id,
                    "previous_assignee": current_assignee,
                    "mode": "auto",
                })
                .to_string(),
                "info",
            )
            .await;
        }
    }
}

/// P6 Theme C: per-tick auto-recovery pass. For each startup with
/// `auto_recovery_enabled = 1`, find tasks in `changes_requested`
/// whose `review_round >= auto_recovery_max_attempts` and reassign
/// to an idle same-role peer. Mirrors the `auto_steal_pass` plumbing
/// — the SQL gate is `assignee = old_assignee AND review_round >=
/// threshold AND status = 'changes_requested'`, so a concurrent
/// manual `task_steal` between SELECT and UPDATE loses the race
/// cleanly. Emits `task_recovered` system_event with severity info
/// on success.
async fn auto_recovery_pass(
    world: &WorldView,
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
) {
    let startups: Vec<(String, i64)> = sqlx::query_as(
        "SELECT id, auto_recovery_max_attempts FROM startups WHERE auto_recovery_enabled = 1",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    for (startup_id, max_attempts) in startups {
        // Find changes_requested tasks past the threshold.
        let candidates: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT id, assignee_agent_id, review_round FROM tasks \
             WHERE startup_id = ? AND status = 'changes_requested' \
               AND assignee_agent_id IS NOT NULL \
               AND review_round >= ?",
        )
        .bind(&startup_id)
        .bind(max_attempts)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        for (task_id, current_assignee, review_round) in candidates {
            // Look up the current assignee's role for same-role peer
            // pick.
            let assignee_role: Option<String> =
                sqlx::query_scalar("SELECT role FROM agents WHERE id = ?")
                    .bind(&current_assignee)
                    .fetch_optional(pool)
                    .await
                    .unwrap_or(None);
            let Some(role) = assignee_role else { continue };
            // Pick the first idle same-role peer that isn't the
            // current assignee.
            let peer_id = world.avatars.iter().find_map(|(id, a)| {
                if a.startup_id == startup_id
                    && a.role == role
                    && a.status == "idle"
                    && *id != current_assignee
                {
                    Some(id.clone())
                } else {
                    None
                }
            });
            let Some(peer_id) = peer_id else { continue };

            // Reassign + reset to queued. Same-row WHERE clause on
            // assignee + status so we don't clobber a manual steal or
            // a re-flip that happened between SELECT and UPDATE.
            let res = sqlx::query(
                "UPDATE tasks SET assignee_agent_id = ?, status = 'queued', \
                                  review_round = 0, updated_at = unixepoch() \
                 WHERE id = ? AND status = 'changes_requested' \
                   AND assignee_agent_id = ?",
            )
            .bind(&peer_id)
            .bind(&task_id)
            .bind(&current_assignee)
            .execute(pool)
            .await;
            let rows = match res {
                Ok(r) => r.rows_affected(),
                Err(e) => {
                    tracing::warn!(component = "scheduler",
                        task_id = %task_id, error = %e,
                        "auto-recovery UPDATE failed"
                    );
                    continue;
                }
            };
            if rows == 0 {
                continue;
            }
            let _ = crate::persist::append_audit(
                pool,
                &task_id,
                &json!({
                    "actor": "scheduler",
                    "kind": "task_recovered",
                    "strategy": "peer_reassign",
                    "from_agent_id": current_assignee,
                    "to_agent_id": peer_id,
                    "review_round_at_recovery": review_round,
                })
                .to_string(),
            )
            .await;
            let _ = crate::emit::emit_system_event(
                pool,
                event_tx,
                Some(&startup_id),
                "task_recovered",
                &json!({
                    "task_id": task_id,
                    "strategy": "peer_reassign",
                    "from_agent_id": current_assignee,
                    "to_agent_id": peer_id,
                    "review_round_at_recovery": review_round,
                })
                .to_string(),
                "info",
            )
            .await;
        }
    }
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
