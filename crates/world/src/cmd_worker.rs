//! Worker-side command dispatcher. Handles `WorkerInbound` messages from the
//! agent CLIs. Wired in M1.13: only `MoveIntent` is fully handled; other
//! variants are stubbed and reply with a generic `ok` so the WS round-trip
//! still completes during Phase 0 development.

use crate::move_sys::{self, PathStore, StartMoveResult};
use crate::path::RoomGraph;
use crate::protocol::WorkerInbound;
use crate::seed::TownLayout;
use crate::state::WorldView;
use serde_json::json;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub async fn dispatch(
    world: &mut WorldView,
    paths: &mut PathStore,
    layout: &TownLayout,
    graph: &RoomGraph,
    out_bus: &HashMap<String, mpsc::Sender<serde_json::Value>>,
    agent_id: &str,
    msg: serde_json::Value,
) -> serde_json::Value {
    let inbound: WorkerInbound = match serde_json::from_value(msg) {
        Ok(v) => v,
        Err(e) => return json!({"type":"error","reason":"parse","detail":e.to_string()}),
    };
    match inbound {
        WorkerInbound::Hello { .. } => json!({"type":"ok","kind":"hello"}),
        WorkerInbound::MoveIntent { target_room, target_x, target_y, .. } => {
            let r = move_sys::start_move(
                world, paths, layout, graph, agent_id, &target_room, target_x, target_y,
            );
            match r {
                StartMoveResult::Ok => json!({"type":"ok","kind":"move_intent"}),
                StartMoveResult::NoPath => {
                    if let Some(tx) = out_bus.get(agent_id) {
                        let _ = tx.try_send(json!({
                            "type":"move_failed","v":1,"reason":"no_path"
                        }));
                    }
                    json!({"type":"error","reason":"no_path"})
                }
                StartMoveResult::PermissionDenied => {
                    if let Some(tx) = out_bus.get(agent_id) {
                        let _ = tx.try_send(json!({
                            "type":"move_failed","v":1,"reason":"permission_denied"
                        }));
                    }
                    json!({"type":"error","reason":"permission_denied"})
                }
                StartMoveResult::NoSuchAgent => json!({"type":"error","reason":"unknown_agent"}),
            }
        }
        // Stubs — M2/M3 wires the rest.
        _ => json!({"type":"ok","kind":"stub"}),
    }
}
