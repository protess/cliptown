use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Serialize, Deserialize, TS, Clone)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerInbound {
    Hello { v: u8, agent_id: String, startup_id: String, secret: String },
    McpCall { v: u8, corr_id: String, tool: String, args: serde_json::Value },
    ReportBudget { v: u8, in_tokens: u64, out_tokens: u64, model_id: String, task_id: Option<String> },
    ReportFsOp { v: u8, op: String, path: String, bytes: i64, ok: bool, error: Option<String> },
    CliSessionStarted { v: u8, task_id: Option<String>, prompt_hash: String },
    CliSessionEnded { v: u8, task_id: Option<String>, exit_code: i32, summary: Option<String> },
    TaskProgress { v: u8, task_id: String, note: String },
    MoveIntent { v: u8, target_room: String, target_x: i32, target_y: i32 },
}

#[derive(Debug, Serialize, Deserialize, TS, Clone)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerOutbound {
    McpReply { v: u8, corr_id: String, result: serde_json::Value },
    McpError { v: u8, corr_id: String, code: String, message: String },
    WorldState { v: u8, snapshot: serde_json::Value },
    WorldStateChunk { v: u8, seq: u32, total: u32, payload: serde_json::Value },
    WorldStateEnd { v: u8 },
    TaskAssigned { v: u8, task_id: String, title: String, description: String, required_room: Option<String>, parent_id: Option<String> },
    SubtaskProposed { v: u8, parent_id: String, proposed_task_id: String, proposer_agent_id: String, title: String, description: String, suggested_assignee_role: Option<String> },
    SubtaskDone { v: u8, parent_id: String, child_id: String, artifact_path: String, review_round: u32 },
    Directive { v: u8, from_agent_id: String, body: String, in_response_to_task: Option<String> },
    ProximityTick { v: u8, room_id: String, members: Vec<serde_json::Value> },
    ChatReceived { v: u8, from_agent_id: String, body: String, room_id: String },
    MoveComplete { v: u8, room_id: String },
    MoveFailed { v: u8, reason: String },
    BudgetWarning { v: u8, remaining_usd: f64, percent_used: u32 },
    Pause { v: u8, reason: String },
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize, TS, Clone)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConsoleInbound {
    Hello { v: u8, operator_token: String },
    OperatorMove { v: u8, target_x: i32, target_y: i32 },
    OperatorPossess { v: u8, startup_id: String },
    OperatorUnpossess { v: u8 },
    OperatorDirective { v: u8, to_agent_id: String, body: String },
    OperatorAcceptProposal { v: u8, task_id: String, assignee_agent_id: String, required_room: Option<String> },
    OperatorRejectProposal { v: u8, task_id: String, reason: String },
    OperatorForceAccept { v: u8, task_id: String },
    OperatorForceFail { v: u8, task_id: String, note: String },
    OperatorRecheckBackends,
}

#[derive(Debug, Serialize, Deserialize, TS, Clone)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConsoleOutbound {
    WorldViewSnapshot { v: u8, snapshot: serde_json::Value },
    WorldViewDelta { v: u8, tick_seq: u64, changes: serde_json::Value },
    SystemEvent { v: u8, severity: String, kind: String, startup_id: Option<String>, payload: serde_json::Value, ts: i64 },
    BackendCatalog { v: u8, entries: serde_json::Value },
    Toast { v: u8, severity: String, body: String, sticky: bool },
    Modal { v: u8, kind: String, payload: serde_json::Value },
    /// Operator-visible chat message. Always room-scoped. `ts` is UNIX MILLISECONDS
    /// (matches frontend `new Date(m.ts)` rendering convention; SQL `messages.ts`
    /// stores seconds and gets multiplied at the emit site).
    Chat {
        v: u8,
        message_id: String,
        ts: i64,
        startup_id: String,
        room_id: String,
        author_id: String,
        body: String,
    },
    /// Operator-visible directive. Room-independent. `author_id` is the sentinel
    /// "operator" for operator-sourced directives, real `agent_id` for peer- or
    /// manager-sourced. `in_response_to_task` is `Some(task_id)` only for review-
    /// cycle feedback (mcp_dispatch::handle_task_request_changes). `ts` is UNIX
    /// MILLISECONDS, see `Chat` doc above.
    Directive {
        v: u8,
        message_id: String,
        ts: i64,
        startup_id: String,
        author_id: String,
        to_agent_id: String,
        body: String,
        in_response_to_task: Option<String>,
    },
}
