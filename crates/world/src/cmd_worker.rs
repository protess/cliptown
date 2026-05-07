//! Worker-side command dispatcher. Handles `WorkerInbound` messages from the
//! agent CLIs. M1.13 wired `MoveIntent`; M1.15 wires `ReportBudget` (cost
//! tracking + 80/95/100 thresholds + pause-all). Other variants remain stubbed
//! so the WS round-trip still completes during Phase 0 development.

use crate::move_sys::{self, PathStore, StartMoveResult};
use crate::path::RoomGraph;
use crate::protocol::WorkerInbound;
use crate::seed::TownLayout;
use crate::state::WorldView;
use serde_json::json;
use sqlx::SqlitePool;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub async fn dispatch(
    world: &mut WorldView,
    paths: &mut PathStore,
    layout: &TownLayout,
    graph: &RoomGraph,
    out_bus: &HashMap<String, mpsc::Sender<serde_json::Value>>,
    pool: &SqlitePool,
    agent_id: &str,
    msg: serde_json::Value,
) -> serde_json::Value {
    let inbound: WorkerInbound = match serde_json::from_value(msg) {
        Ok(v) => v,
        Err(e) => return json!({"type":"error","reason":"parse","detail":e.to_string()}),
    };
    match inbound {
        WorkerInbound::Hello { .. } => json!({"type":"ok","kind":"hello"}),
        WorkerInbound::ReportBudget { in_tokens, out_tokens, model_id, task_id, .. } => {
            let startup_id = match world.avatars.get(agent_id) {
                Some(a) => a.startup_id.clone(),
                None => return json!({"type":"error","reason":"unknown_agent"}),
            };
            match crate::budget::apply_report(
                pool,
                &startup_id,
                agent_id,
                task_id.as_deref(),
                &model_id,
                in_tokens,
                out_tokens,
            )
            .await
            {
                Ok((new_spent, cap, threshold)) => {
                    if let Some(t) = threshold {
                        if let Err(e) = crate::budget::record_threshold_event(
                            pool,
                            &startup_id,
                            t,
                            new_spent,
                            cap,
                        )
                        .await
                        {
                            tracing::warn!(error = %e, "record_threshold_event failed");
                        }
                        if matches!(t, crate::budget::Threshold::Pause100) {
                            crate::budget::pause_startup(world, out_bus, &startup_id);
                        }
                        // Console toast plumbing for warn80/warn95 lives in M2
                        // (system_event broadcast). The system_events row above
                        // is the durable record; the console event feed will
                        // surface it.
                    }
                    json!({"type":"ok","kind":"report_budget","spent_usd":new_spent,"cap_usd":cap})
                }
                Err(e) => json!({"type":"error","reason":"sql","detail":e.to_string()}),
            }
        }
        WorkerInbound::MoveIntent { target_room, target_x, target_y, .. } => {
            let r = move_sys::start_move(
                world, paths, layout, graph, agent_id, &target_room, target_x, target_y,
            );
            match r {
                StartMoveResult::Ok => json!({"type":"ok","kind":"move_intent"}),
                StartMoveResult::NoPath => {
                    if let Some(tx) = out_bus.get(agent_id) {
                        let payload = json!({
                            "type":"move_failed","v":1,"reason":"no_path"
                        });
                        if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) =
                            tx.try_send(payload)
                        {
                            tracing::warn!(agent_id = %agent_id, "out_bus full, dropping move_failed (no_path)");
                        }
                    }
                    json!({"type":"error","reason":"no_path"})
                }
                StartMoveResult::PermissionDenied => {
                    if let Some(tx) = out_bus.get(agent_id) {
                        let payload = json!({
                            "type":"move_failed","v":1,"reason":"no_permission"
                        });
                        if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) =
                            tx.try_send(payload)
                        {
                            tracing::warn!(agent_id = %agent_id, "out_bus full, dropping move_failed (no_permission)");
                        }
                    }
                    json!({"type":"error","reason":"no_permission"})
                }
                StartMoveResult::NoSuchAgent => json!({"type":"error","reason":"unknown_agent"}),
            }
        }
        // Stubs — M2/M3 wires the rest.
        _ => json!({"type":"ok","kind":"stub"}),
    }
}
