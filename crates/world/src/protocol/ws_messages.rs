use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Serialize, Deserialize, TS, Clone)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerInbound {
    Hello { v: u8, agent_id: String, startup_id: String, secret: String },
    McpCall { v: u8, corr_id: String, tool: String, args: serde_json::Value },
    /// Worker tells the world about token spend on a CLI turn. When the
    /// adapter could scrape an authoritative cost from the CLI (e.g.
    /// claude-code's `total_cost_usd`), `cost_usd` carries it and the world
    /// uses it directly. When absent, the world falls back to its hardcoded
    /// `budget::price_per_mtok` table keyed on `model_id`.
    ReportBudget {
        v: u8,
        in_tokens: u64,
        out_tokens: u64,
        model_id: String,
        task_id: Option<String>,
        #[serde(default)]
        cost_usd: Option<f64>,
    },
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
    TaskAssigned { v: u8, task_id: String, title: String, description: String, required_room: Option<String>, parent_id: Option<String>, preferred_backend: Option<String>, preferred_model: Option<String> },
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
    /// P2.2 operator-side skills management (attach/detach only; upsert/
    /// delete still go through MCP tools). `startup_id` is explicit so the
    /// operator can manage skills across startups without re-possessing.
    SkillAttach { v: u8, startup_id: String, agent_id: String, skill_id: String },
    SkillDetach { v: u8, startup_id: String, agent_id: String, skill_id: String },
    /// P3 Theme F follow-up: operator-side authoring. `skill_id` is the
    /// upsert target — pass None to create. Manager-or-above; viewers can't
    /// reshape content. Mirrors the agent-side `skill_upsert` MCP tool but
    /// scoped by the operator's possessed startup.
    SkillUpsertOperator { v: u8, startup_id: String, skill_id: Option<String>, name: String, content_md: String },
    SkillDeleteOperator { v: u8, startup_id: String, skill_id: String },
    /// P3 carry-forward: admin-only flag flip. When set, the skill auto-
    /// surfaces in every agent's execenv regardless of `agent_skills`
    /// attachment.
    SkillSetGlobal { v: u8, skill_id: String, is_global: bool },
    /// P3 Theme B follow-up: admin-only operator-management commands. The
    /// `operators` table has been in place since #52 with role gating wired
    /// through dispatch; these complete the surface so an admin can
    /// provision additional operators without touching SQL.
    OperatorList { v: u8 },
    OperatorCreate { v: u8, name: String, role: String },
    OperatorRevoke { v: u8, operator_id: String },
    OperatorSetRole { v: u8, operator_id: String, role: String },
    /// P4 Theme E1: admin-only flip on an agent's peer-reviewer flag.
    /// When set, the agent can request changes on any same-startup task
    /// (except their own) via `task_request_changes`.
    AgentSetPeerReviewer { v: u8, agent_id: String, is_peer_reviewer: bool },
    /// P4 Theme E3: admin-only per-startup auto-steal flag. When enabled,
    /// the scheduler runs the auto-steal pass each tick, reassigning
    /// queued tasks whose assignee is busy past `after_secs` to an idle
    /// same-role peer. `after_secs` is optional — when `None`, the
    /// SQL default (60s) is left in place.
    StartupSetAutoSteal { v: u8, startup_id: String, enabled: bool, after_secs: Option<i64> },
    /// P4 Theme G slice 4: operator-side skill revision surface.
    /// `list` is read-only (any logged-in operator); `revert` is a write
    /// (manager-or-above). Same-startup gate enforced server-side via
    /// the existing `skills::list_revisions` / `skills::revert_to_revision`
    /// helpers. The agent-side MCP tools (`skill_list_revisions`,
    /// `skill_revert`) remain — these are the operator-console twins.
    SkillListRevisionsOperator { v: u8, startup_id: String, skill_id: String },
    SkillRevertOperator { v: u8, startup_id: String, skill_id: String, rev_seq: i64 },
    /// P5 Theme A: operator presence heartbeat. Frontend emits on
    /// startup-click (focus change) and on a 30s tick. Server upserts
    /// the registry entry and re-broadcasts presence if anything
    /// changed. `focused_startup_id == None` means the operator has no
    /// startup focused (e.g. on the empty sidebar).
    PresenceHeartbeat { v: u8, focused_startup_id: Option<String> },
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
        /// P5 Theme B: operator-sourced directives carry the operator's
        /// display name resolved at emit time. `None` for agent-sourced
        /// directives (peer review etc.) where the frontend can look up
        /// the agent by `author_id` directly. Server-side resolution
        /// means a name change in `operators` is reflected in *future*
        /// broadcasts; historical frames keep the name they were emitted
        /// with.
        author_display_name: Option<String>,
        to_agent_id: String,
        body: String,
        in_response_to_task: Option<String>,
    },
    /// P2.2 broadcast on every skills mutation. `kind` is "upsert", "delete",
    /// "attach", or "detach". `agent_id` is `Some` for attach/detach.
    /// For `kind="upsert"` the `skill` field carries the listing row
    /// (`{id, name, len, updated_at, attachments}`) so the frontend can apply
    /// in place without a follow-up fetch.
    SkillChanged {
        v: u8,
        startup_id: String,
        kind: String,
        skill_id: String,
        agent_id: Option<String>,
        skill: Option<serde_json::Value>,
    },
    /// P2.2 initial state delivery — emitted at console connect after the
    /// WorldViewSnapshot. `startups` is `{sid: [{id, name, len, updated_at,
    /// attachments: [agent_id]}]}` for every startup the operator can see.
    SkillsSnapshot { v: u8, startups: serde_json::Value },
    /// P3 carry-forward: identity of the authenticated operator. Emitted
    /// once after a successful WS hello — lets the frontend hide admin-only
    /// surfaces (OperatorsPanel, the SkillsPanel global toggle) for
    /// viewer / manager operators. The wire never echoes the bearer token.
    HelloOk { v: u8, operator_id: String, operator_name: String, role: String },
    /// P5 Theme A: operator presence list. Re-broadcast on
    /// connect/disconnect/heartbeat-with-changed-focus and on each GC
    /// tick that drops entries. `presences` is a JSON array of
    /// `{operator_id, operator_name, role, focused_startup_id?,
    /// last_seen_at}`. Untyped JsonValue here keeps presence::PresenceEntry
    /// from cascading through the ts-rs export — the frontend store
    /// coerces defensively.
    OperatorPresence { v: u8, presences: serde_json::Value },
    /// P5 Theme C: a destructive action just acquired a soft-lock.
    /// `info` carries `{lock_key, operator_id, operator_name,
    /// expires_at}`. Peers should disable the matching UI affordance
    /// until they see `ActionUnlocked` or `expires_at` passes.
    ActionLocked { v: u8, info: serde_json::Value },
    /// P5 Theme C: a soft-lock was released (success or TTL expiry).
    ActionUnlocked { v: u8, lock_key: String },
}
