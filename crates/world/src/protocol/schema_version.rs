use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
pub struct SchemaVersion { pub v: u8 }

impl SchemaVersion { pub const CURRENT: Self = Self { v: 1 }; }
