//! Per-viewer world_view snapshot construction + chunked transport.
//! Workers receive an agent-scoped snapshot; operators (M1.12) receive a
//! broader god-view snapshot. Snapshots > CHUNK_THRESHOLD are split into
//! multiple WorldStateChunk frames terminated by WorldStateEnd.

use crate::state::{AvatarView, WorldView};
use serde::Serialize;
use std::collections::HashMap;

/// Below this size in bytes, the snapshot is sent as a single `WorldState` frame.
/// Above, it is split into `WorldStateChunk` frames + `WorldStateEnd`.
pub const CHUNK_THRESHOLD: usize = 256 * 1024; // 256 KiB

/// Per-payload chunk size (bytes of UTF-8 JSON). Tuned to keep frame sizes well
/// under common WS frame limits while minimizing chunk count.
pub const CHUNK_SIZE: usize = 64 * 1024; // 64 KiB

/// Maximum number of same-room peers included in a worker snapshot.
pub const PEER_CAP: usize = 16;

/// Maximum number of recent messages (own startup) included in a worker snapshot.
pub const MESSAGE_CAP: usize = 20;

#[derive(Debug, Clone, Serialize)]
pub struct WorkerSnapshot<'a> {
    pub agent_id: &'a str,
    pub startup_id: &'a str,
    pub own_avatar: Option<&'a AvatarView>,
    pub peers_in_room: Vec<&'a AvatarView>,
    pub recent_messages: Vec<MessageStub>,
    pub backend_catalog: &'a HashMap<String, serde_json::Value>,
    pub current_task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct MessageStub {
    pub id: String,
    pub author_id: String,
    pub body: String,
    pub kind: String,
    pub ts: i64,
}

/// Build a worker-scoped snapshot from the live `WorldView`.
/// `recent_messages` is supplied by the caller (since they live in SQLite, not WorldView).
pub fn build_worker_snapshot<'a>(
    world: &'a WorldView,
    agent_id: &'a str,
    startup_id: &'a str,
    recent_messages: Vec<MessageStub>,
    current_task_id: Option<String>,
) -> WorkerSnapshot<'a> {
    let own_avatar = world.avatars.get(agent_id);
    let own_room = own_avatar.map(|a| a.room_id.as_str());

    let peers_in_room: Vec<&AvatarView> = match own_room {
        None => Vec::new(),
        Some(room) => world
            .avatars
            .values()
            .filter(|a| a.agent_id != agent_id && a.room_id == room)
            .take(PEER_CAP)
            .collect(),
    };

    let recent_messages = if recent_messages.len() > MESSAGE_CAP {
        recent_messages
            .into_iter()
            .rev()
            .take(MESSAGE_CAP)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        recent_messages
    };

    WorkerSnapshot {
        agent_id,
        startup_id,
        own_avatar,
        peers_in_room,
        recent_messages,
        backend_catalog: &world.backend_catalog,
        current_task_id,
    }
}

/// Decide whether a snapshot needs chunking, and produce the JSON-serialized
/// chunks. Returns Vec of [`ChunkFrame`]s. When the input fits in one frame, the
/// Vec has length 1 with seq=0 total=1.
pub fn chunk_snapshot<S: Serialize>(snapshot: &S) -> anyhow::Result<Vec<ChunkFrame>> {
    let json = serde_json::to_string(snapshot)?;
    let total_bytes = json.len();

    if total_bytes <= CHUNK_THRESHOLD {
        return Ok(vec![ChunkFrame {
            seq: 0,
            total: 1,
            payload: json,
        }]);
    }

    // Slice the JSON byte string into ~CHUNK_SIZE windows, but back up each
    // window's end to the nearest UTF-8 boundary so every chunk's payload is a
    // valid UTF-8 string. The receiver concatenates payloads to reconstruct the
    // original JSON byte-for-byte before parsing.
    let bytes = json.as_bytes();
    let mut frames: Vec<ChunkFrame> = Vec::new();
    let mut start = 0usize;
    let mut seq = 0u32;
    while start < bytes.len() {
        // Aim for end = start + CHUNK_SIZE, but back up to a UTF-8 boundary.
        let mut end = (start + CHUNK_SIZE).min(bytes.len());
        // A continuation byte has the bit pattern 10xxxxxx. If `end` lands on
        // one (and isn't past the buffer), step back until we're at the start
        // of a codepoint.
        while end < bytes.len() && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
            end -= 1;
        }
        let payload = std::str::from_utf8(&bytes[start..end])?.to_string();
        frames.push(ChunkFrame {
            seq,
            total: 0, // patched below once we know the count
            payload,
        });
        start = end;
        seq += 1;
    }
    let total = frames.len() as u32;
    for f in frames.iter_mut() {
        f.total = total;
    }
    Ok(frames)
}

#[derive(Debug, Clone)]
pub struct ChunkFrame {
    pub seq: u32,
    pub total: u32,
    pub payload: String,
}

/// Reassemble chunk payloads on the receiver side and parse the result.
/// Used by tests; the worker-side TS will reimplement this.
pub fn reassemble<T: serde::de::DeserializeOwned>(frames: &[ChunkFrame]) -> anyhow::Result<T> {
    if frames.is_empty() {
        anyhow::bail!("empty frames");
    }
    let total = frames[0].total;
    if frames.len() as u32 != total {
        anyhow::bail!(
            "frame count mismatch: got {} expected {}",
            frames.len(),
            total
        );
    }
    let combined: String = frames.iter().map(|f| f.payload.as_str()).collect();
    Ok(serde_json::from_str(&combined)?)
}
