//! Per-tick proximity broadcast. Groups avatars by room and pushes a
//! `proximity_tick` event to each member's out_bus.

use crate::state::WorldView;
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Build proximity payloads per room and push to each member's out_bus.
/// Phase 0: cross-startup peers ARE included — invariant 7 (public rooms).
/// The worker is responsible for filtering as needed.
pub fn compute_and_emit(
    world: &WorldView,
    out_bus: &HashMap<String, mpsc::Sender<Value>>,
) {
    // Group avatars by room.
    let mut rooms: HashMap<String, Vec<&crate::state::AvatarView>> = HashMap::new();
    for avatar in world.avatars.values() {
        rooms.entry(avatar.room_id.clone()).or_default().push(avatar);
    }
    // For each room, build a peers array and push to each member.
    for (room_id, members) in &rooms {
        if members.is_empty() {
            continue;
        }
        let peers: Vec<Value> = members
            .iter()
            .map(|a| {
                json!({
                    "agent_id": a.agent_id,
                    "role": a.role,
                    "startup_id": a.startup_id,
                })
            })
            .collect();
        for a in members {
            if let Some(tx) = out_bus.get(&a.agent_id) {
                let payload = json!({
                    "type": "proximity_tick",
                    "v": 1,
                    "room_id": room_id,
                    "peers": peers,
                });
                if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(payload) {
                    tracing::warn!(agent_id = %a.agent_id, "out_bus full, dropping proximity_tick");
                }
            }
        }
    }
}
