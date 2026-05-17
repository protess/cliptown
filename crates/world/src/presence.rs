//! P5 Theme A: operator presence registry.
//!
//! Tracks which operators are currently connected to the console WS and
//! which startup each is focused on. Updated by `http::handle_console`
//! on connect/disconnect and by inbound `presence_heartbeat` frames. A
//! periodic Tokio task (spawned by `loop_::spawn_with_layout`) drops
//! entries whose `last_seen_at` is past the TTL.
//!
//! Wire surface: `ConsoleOutbound::OperatorPresence { v, presences }`
//! re-emitted whenever the registry mutates. Frontend `WorldState`
//! stores the most recent presence list and renders avatars in the
//! Sidebar + TopBar.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 3× the 30s heartbeat — three missed beats and the entry drops.
pub const PRESENCE_TTL_SECS: i64 = 90;

#[derive(Debug, Clone, Serialize)]
pub struct PresenceEntry {
    pub operator_id: String,
    pub operator_name: String,
    pub role: String,
    pub focused_startup_id: Option<String>,
    pub last_seen_at: i64,
}

pub type PresenceRegistry = Arc<RwLock<HashMap<String, PresenceEntry>>>;

pub fn new_registry() -> PresenceRegistry {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Snapshot the current registry into a serializable Vec.
pub async fn snapshot(reg: &PresenceRegistry) -> Vec<PresenceEntry> {
    reg.read().await.values().cloned().collect()
}

/// Upsert an entry. Used on connect and on heartbeat. Returns true if
/// the broadcast snapshot should be re-emitted (always true for new
/// connections; for heartbeats only when `focused_startup_id`
/// changed — saves a broadcast on every 30s tick).
pub async fn upsert(
    reg: &PresenceRegistry,
    operator_id: &str,
    operator_name: &str,
    role: &str,
    focused_startup_id: Option<String>,
    now: i64,
) -> bool {
    let mut g = reg.write().await;
    match g.get_mut(operator_id) {
        Some(existing) => {
            existing.last_seen_at = now;
            if existing.focused_startup_id != focused_startup_id {
                existing.focused_startup_id = focused_startup_id;
                true
            } else {
                false
            }
        }
        None => {
            g.insert(
                operator_id.to_string(),
                PresenceEntry {
                    operator_id: operator_id.to_string(),
                    operator_name: operator_name.to_string(),
                    role: role.to_string(),
                    focused_startup_id,
                    last_seen_at: now,
                },
            );
            true
        }
    }
}

/// Drop an entry by id. Returns true if it existed.
pub async fn drop_entry(reg: &PresenceRegistry, operator_id: &str) -> bool {
    reg.write().await.remove(operator_id).is_some()
}

/// Drop entries whose `last_seen_at` is older than `now - PRESENCE_TTL_SECS`.
/// Returns the count of dropped entries so the caller can decide whether
/// to emit a fresh broadcast.
pub async fn gc(reg: &PresenceRegistry, now: i64) -> usize {
    let cutoff = now - PRESENCE_TTL_SECS;
    let mut g = reg.write().await;
    let before = g.len();
    g.retain(|_, e| e.last_seen_at >= cutoff);
    before - g.len()
}
