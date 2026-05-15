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
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    agent_id: &str,
    msg: serde_json::Value,
) -> serde_json::Value {
    // M2.3: MCP frames have their own envelope (`mcp_call` → `mcp_reply` /
    // `mcp_error`) and are dispatched by `mcp_dispatch`. Sniffing the type
    // first means the existing `WorkerInbound` deserializer doesn't have to
    // know about every MCP tool variant.
    if msg.get("type") == Some(&serde_json::Value::String("mcp_call".to_string())) {
        return crate::mcp_dispatch::dispatch(
            world, paths, layout, graph, out_bus, pool, event_tx, agent_id, msg,
        )
        .await;
    }
    let inbound: WorkerInbound = match serde_json::from_value(msg) {
        Ok(v) => v,
        Err(e) => return json!({"type":"error","reason":"parse","detail":e.to_string()}),
    };
    match inbound {
        WorkerInbound::Hello { .. } => json!({"type":"ok","kind":"hello"}),
        WorkerInbound::ReportBudget { in_tokens, out_tokens, model_id, task_id, cost_usd, .. } => {
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
                cost_usd,
            )
            .await
            {
                Ok((new_spent, cap, threshold)) => {
                    // P3 Theme C follow-up: cost variance telemetry. When the
                    // task has a `cost_estimate_usd` and this report has a
                    // concrete `cost_usd`, compare actual vs estimate and
                    // emit `task_cost_variance` if delta crosses ±50%. The
                    // 50% threshold is a heuristic — at typical haiku cost
                    // ranges ($0.001–$0.05) anything tighter would flap. We
                    // emit on every crossing rather than only the first
                    // because a task may span multiple report_budget calls
                    // (multi-spawn / resumed runs); the operator console can
                    // dedupe by task_id.
                    if let (Some(tid), Some(actual)) = (task_id.as_deref(), cost_usd) {
                        let est_row: Result<Option<(Option<f64>,)>, _> =
                            sqlx::query_as("SELECT cost_estimate_usd FROM tasks WHERE id = ?")
                                .bind(tid)
                                .fetch_optional(pool)
                                .await;
                        if let Ok(Some((Some(estimate),))) = est_row {
                            if estimate > 0.0 && actual.is_finite() {
                                let delta_pct = (actual - estimate) / estimate * 100.0;
                                if delta_pct.abs() >= 50.0 {
                                    let severity = if delta_pct > 0.0 { "warn" } else { "info" };
                                    let _ = crate::emit::emit_system_event(
                                        pool,
                                        event_tx,
                                        Some(&startup_id),
                                        "task_cost_variance",
                                        &json!({
                                            "task_id": tid,
                                            "estimate_usd": estimate,
                                            "actual_usd": actual,
                                            "delta_pct": (delta_pct * 100.0).round() / 100.0,
                                        })
                                        .to_string(),
                                        severity,
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                    if let Some(t) = threshold {
                        crate::budget::record_threshold_event(
                            pool,
                            event_tx,
                            &startup_id,
                            t,
                            new_spent,
                            cap,
                        )
                        .await;
                        match t {
                            crate::budget::Threshold::Warn80
                            | crate::budget::Threshold::Warn95 => {
                                // Spec §6.1: emit `budget_warning` to all
                                // same-startup workers so they can throttle
                                // before the 100% pause. The system_events
                                // row above is the durable console record.
                                let percent = if cap > 0.0 {
                                    ((new_spent / cap) * 100.0) as u32
                                } else {
                                    0
                                };
                                let remaining = (cap - new_spent).max(0.0);
                                crate::budget::warn_startup(
                                    world,
                                    out_bus,
                                    &startup_id,
                                    remaining,
                                    percent,
                                );
                            }
                            crate::budget::Threshold::Pause100 => {
                                crate::budget::pause_startup(world, out_bus, &startup_id);
                            }
                        }
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
                            tracing::warn!(component = "cmd_worker", agent_id = %agent_id, "out_bus full, dropping move_failed (no_path)");
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
                            tracing::warn!(component = "cmd_worker", agent_id = %agent_id, "out_bus full, dropping move_failed (no_permission)");
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
