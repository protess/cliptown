use std::collections::HashMap;
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Default, Clone, Serialize, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
pub struct WorldView {
    pub tick_seq: u64,
    pub backend_catalog: HashMap<String, serde_json::Value>,
    pub avatars: HashMap<String, AvatarView>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
pub struct AvatarView {
    pub agent_id: String,
    pub startup_id: String,
    pub role: String,
    pub backend: String,
    pub current_pos: (i32, i32),
    pub target_pos: Option<(i32, i32)>,
    pub room_id: String,
    pub status: String,
}
