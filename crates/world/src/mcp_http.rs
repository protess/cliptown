//! M9.10 A1' — MCP-over-HTTP transport at the world.
//!
//! Speaks the JSON-RPC 2.0 wire format used by Anthropic's `claude` CLI (and
//! other MCP clients) configured with `mcp.json` `{type:"http", url, headers}`.
//! Three RPC methods are wired:
//!
//!   - `initialize`   — handshake: protocolVersion + capabilities + serverInfo.
//!   - `tools/list`   — enumerate the 16 cliptown tools with minimal schemas.
//!   - `tools/call`   — translate `{name, arguments}` into the internal
//!                       `mcp_call` envelope, route through the world loop via
//!                       `Cmd::HandleWorkerMsg` (same path workers use over WS),
//!                       and unwrap the `mcp_reply` / `mcp_error` into MCP
//!                       content shape.
//!
//! Auth: `Authorization: Bearer <agent_id>:<secret>`. The agent_id half lets
//! us call `validate_agent_secret(pool, agent_id, secret)` without scanning
//! all agents. CLI-side configures one token; world-side validates with the
//! existing WS secret env-var contract.
//!
//! Why hand-roll instead of `rmcp`:
//!   - Dispatch is loop-channel-routed (`Cmd::HandleWorkerMsg`), not an
//!     async-fn callable that fits rmcp's `Service` abstraction cleanly.
//!   - Only 3 RPC methods needed, all single-shot request/response. No
//!     sessions, no SSE, no `notifications/*` push.
//!   - rmcp's `transport-streamable-http-server` would add a dep + adapter
//!     layer for behavior we can express in ~200 LoC.

use crate::loop_::Cmd;
use crate::http::AppState;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::oneshot;

const PROTOCOL_VERSION: &str = "2025-03-26";
const SERVER_NAME: &str = "cliptown-world";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// JSON-RPC 2.0 request envelope. `id` is optional — when absent the message
/// is a notification (we return 202-equivalent JSON `{}` without a result).
#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

/// Standard JSON-RPC error codes used here:
///   -32600 Invalid Request   (envelope malformed)
///   -32601 Method not found  (unknown RPC method)
///   -32602 Invalid params    (e.g. tools/call missing `name`)
///   -32603 Internal error    (loop disconnected, etc.)
fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

fn rpc_ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

/// POST /mcp handler. Lives in `http::router` (see `http.rs::router`).
pub async fn handle_request(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // 1. Auth: extract `Authorization: Bearer <agent_id>:<secret>` and
    //    validate against the same `validate_agent_secret` flow the worker
    //    WS uses. No agent_id → 401, bad split → 401, bad secret → 401.
    let agent_id = match authenticate(&s, &headers).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    // 2. Parse the JSON-RPC envelope. Anything malformed yields a JSON-RPC
    //    error response on HTTP 200 (per JSON-RPC 2.0 spec — protocol-level
    //    errors stay inside the envelope, not in the HTTP status).
    let req: JsonRpcRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => {
            return Json(rpc_error(Value::Null, -32600, "invalid JSON")).into_response();
        }
    };
    if req.jsonrpc != "2.0" {
        return Json(rpc_error(
            req.id.unwrap_or(Value::Null),
            -32600,
            "jsonrpc must be \"2.0\"",
        ))
        .into_response();
    }
    let id = req.id.clone().unwrap_or(Value::Null);

    // 3. Method dispatch. Notifications (id absent) for `notifications/*`
    //    return an empty 200 body — Claude Code's MCP client posts
    //    `notifications/initialized` after the handshake.
    match req.method.as_str() {
        "initialize" => Json(rpc_ok(id, handle_initialize())).into_response(),
        "tools/list" => Json(rpc_ok(id, handle_tools_list())).into_response(),
        "tools/call" => {
            let res = handle_tools_call(&s, &agent_id, req.params).await;
            Json(rpc_ok(id, res)).into_response()
        }
        m if m.starts_with("notifications/") => {
            // Per MCP streamable-HTTP transport: notifications get HTTP 202
            // Accepted with an EMPTY body. Returning a JSON-RPC-shaped
            // payload here (even `{}`) trips strict clients — rmcp 0.6+
            // (used by `codex exec --json`) errors out with
            // `Deserialize error: data did not match any variant of
            // untagged enum JsonRpcMessage` on the `initialized`
            // notification handshake. claude-code's client was forgiving
            // about the old `{}` 200; new clients aren't.
            (StatusCode::ACCEPTED, "").into_response()
        }
        other => Json(rpc_error(
            id,
            -32601,
            &format!("method not found: {other}"),
        ))
        .into_response(),
    }
}

/// Parse the bearer token, split on the first ':' to recover (agent_id,
/// secret), and run the existing `validate_agent_secret`. Returns either
/// the agent_id (for downstream dispatch) or a 401 response.
async fn authenticate(
    s: &Arc<AppState>,
    headers: &HeaderMap,
) -> Result<String, Response> {
    let header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = match header.strip_prefix("Bearer ") {
        Some(t) => t,
        None => return Err(unauthorized("missing bearer token")),
    };
    let (agent_id, secret) = match token.split_once(':') {
        Some(t) => t,
        None => return Err(unauthorized("token must be <agent_id>:<secret>")),
    };
    if crate::auth::validate_agent_secret(&s.pool, agent_id, secret)
        .await
        .is_err()
    {
        return Err(unauthorized("invalid agent secret"));
    }
    Ok(agent_id.to_string())
}

fn unauthorized(msg: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthorized", "detail": msg})),
    )
        .into_response()
}

fn handle_initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
    })
}

/// Hand-written catalog. Schemas are permissive — Phase 1 verifies behavior
/// against the existing `mcp_dispatch::dispatch` which does its own arg
/// validation. Future M11 work can codegen tight schemas from the Rust
/// handler signatures (similar to `ts-rs` for ConsoleOutbound types).
fn handle_tools_list() -> Value {
    let tool = |name: &str, desc: &str, schema: Value| {
        json!({
            "name": name,
            "description": desc,
            "inputSchema": schema,
        })
    };
    // The 16 cliptown tools, enumerated in `mcp_dispatch::dispatch`. Order
    // here is the ship-gate canonical order in the spec doc § "What exists
    // today / The world's MCP dispatch handles 16 tools".
    let tools = json!([
        tool(
            "move_intent",
            "Walk the caller toward a target room (and optional tile).",
            json!({
                "type": "object",
                "properties": {
                    "target_room": {"type": "string"},
                    "target_tile": {"type": "object"}
                }
            }),
        ),
        tool(
            "speak",
            "Emit a chat or directive message visible to room peers.",
            json!({
                "type": "object",
                "required": ["body"],
                "properties": {
                    "body": {"type": "string"},
                    "kind": {"type": "string"}
                }
            }),
        ),
        tool(
            "task_done",
            "Submit a completed task with the canonical artifact path.",
            json!({
                "type": "object",
                "required": ["task_id", "artifact_path"],
                "properties": {
                    "task_id": {"type": "string"},
                    "artifact_path": {"type": "string"}
                }
            }),
        ),
        tool(
            "task_failed",
            "Report a task as failed with a reason.",
            json!({
                "type": "object",
                "required": ["task_id", "reason"],
                "properties": {
                    "task_id": {"type": "string"},
                    "reason": {"type": "string"}
                }
            }),
        ),
        tool(
            "subtask_create",
            "Create a subtask under the caller's current task.",
            json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "assignee_agent_id": {"type": "string"}
                }
            }),
        ),
        tool(
            "task_accept",
            "Manager accepts a completed task.",
            json!({"type": "object", "required": ["task_id"]}),
        ),
        tool(
            "task_request_changes",
            "Manager requests changes on a submitted task.",
            json!({
                "type": "object",
                "required": ["task_id"],
                "properties": {
                    "task_id": {"type": "string"},
                    "reason": {"type": "string"}
                }
            }),
        ),
        tool(
            "accept_proposal",
            "Accept a peer-proposed subtask.",
            json!({"type": "object", "required": ["task_id"]}),
        ),
        tool(
            "reject_proposal",
            "Reject a peer-proposed subtask.",
            json!({"type": "object", "required": ["task_id"]}),
        ),
        tool(
            "hypothesis_state",
            "Append an epistemic hypothesis to the task log.",
            json!({"type": "object"}),
        ),
        tool(
            "test_record",
            "Append an epistemic test record to the task log.",
            json!({"type": "object"}),
        ),
        tool(
            "hypothesis_resolve",
            "Resolve an open hypothesis in the task log.",
            json!({"type": "object"}),
        ),
        tool(
            "verify",
            "Run a verifier (read_assert, lint_json, etc.) against an artifact.",
            json!({"type": "object"}),
        ),
        tool(
            "ask_peer",
            "Ask a same-room peer a question (Phase 0 stub).",
            json!({"type": "object"}),
        ),
        tool(
            "observe_world",
            "Snapshot the caller's room context.",
            json!({"type": "object"}),
        ),
        tool(
            "read_artifact",
            "Read an artifact path inside the caller's sandbox.",
            json!({"type": "object"}),
        ),
        tool(
            "skill_upsert",
            "Author or update a workspace-scoped markdown skill.",
            json!({
                "type": "object",
                "properties": {
                    "name":       {"type": "string"},
                    "content_md": {"type": "string"}
                },
                "required": ["name", "content_md"]
            }),
        ),
        tool(
            "skill_list",
            "List skills in the caller's startup (metadata only, no content).",
            json!({"type": "object", "properties": {}}),
        ),
        tool(
            "skill_attach",
            "Attach a skill to an agent in the caller's startup.",
            json!({
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "skill_id": {"type": "string"}
                },
                "required": ["agent_id", "skill_id"]
            }),
        ),
        tool(
            "skill_detach",
            "Detach a skill from an agent. Idempotent.",
            json!({
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "skill_id": {"type": "string"}
                },
                "required": ["agent_id", "skill_id"]
            }),
        ),
        tool(
            "skill_delete",
            "Delete a skill. Cascades to all attachments.",
            json!({
                "type": "object",
                "properties": {
                    "skill_id": {"type": "string"}
                },
                "required": ["skill_id"]
            }),
        ),
        tool(
            "skill_file_upsert",
            "Upsert a text file attached to a skill. Materializes at `<workdir>/skills/<skill-name>/<name>`.",
            json!({
                "type": "object",
                "required": ["skill_id", "name", "content"],
                "properties": {
                    "skill_id": {"type": "string"},
                    "name": {"type": "string"},
                    "content": {"type": "string"}
                }
            }),
        ),
        tool(
            "skill_file_delete",
            "Delete a single file attached to a skill (by file name). Idempotent.",
            json!({
                "type": "object",
                "required": ["skill_id", "name"],
                "properties": {
                    "skill_id": {"type": "string"},
                    "name": {"type": "string"}
                }
            }),
        ),
        tool(
            "skill_list_revisions",
            "List historical revisions of a skill, newest first. Caller must own the skill's startup.",
            json!({
                "type": "object",
                "required": ["skill_id"],
                "properties": {
                    "skill_id": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100}
                }
            }),
        ),
        tool(
            "task_set_blocking",
            "Set per-task blocking dependency + deadline. Manager-or-assignee only; null clears either field. Editing deadline clears the overdue dedup so the new boundary fires fresh.",
            json!({
                "type": "object",
                "required": ["task_id"],
                "properties": {
                    "task_id": {"type": "string"},
                    "blocked_on": {"type": ["string", "null"]},
                    "deadline_at": {"type": ["integer", "null"]}
                }
            }),
        ),
        tool(
            "skill_revert",
            "Revert a skill's content_md to a previous revision (by rev_seq). Appends a new revision pointing at the historical content; history stays linear.",
            json!({
                "type": "object",
                "required": ["skill_id", "rev_seq"],
                "properties": {
                    "skill_id": {"type": "string"},
                    "rev_seq": {"type": "integer", "minimum": 1}
                }
            }),
        ),
        tool(
            "task_set_preference",
            "Set per-task model routing override (preferred_backend / preferred_model). Manager-or-assignee only; null clears.",
            json!({
                "type": "object",
                "required": ["task_id"],
                "properties": {
                    "task_id": {"type": "string"},
                    "preferred_backend": {"type": ["string", "null"]},
                    "preferred_model": {"type": ["string", "null"]}
                }
            }),
        ),
        tool(
            "task_steal",
            "Claim a queued task currently assigned to an idle peer. Caller must be idle, in the same startup, share the assignee's role, and not be the current assignee.",
            json!({
                "type": "object",
                "required": ["task_id"],
                "properties": {
                    "task_id": {"type": "string"}
                }
            }),
        ),
        tool(
            "self_review",
            "Run the pre-submit self-review pipeline on this task's artifact. Returns {ok, must_fix:[{check,severity,message}]}. Severity 'error' blocks task_done; 'warn' is informational.",
            json!({
                "type": "object",
                "required": ["task_id", "artifact_path"],
                "properties": {
                    "task_id": {"type": "string"},
                    "artifact_path": {"type": "string"}
                }
            }),
        ),
        tool(
            "run_tests",
            "Run the task's test suite inside the per-task workdir. Optional `command` (e.g. 'pnpm test', 'cargo test --quiet') overrides auto-detect. Returns {exit_code, stdout_tail, stderr_tail, timed_out, elapsed_ms}.",
            json!({
                "type": "object",
                "required": ["task_id"],
                "properties": {
                    "task_id": {"type": "string"},
                    "command": {"type": "string"},
                    "timeout_secs": {"type": "integer", "minimum": 1, "maximum": 600}
                }
            }),
        ),
        tool(
            "lint_artifact",
            "Lint the task's artifact. Dispatches by extension: .ts/.tsx → tsc --noEmit, .rs → cargo check, .md → markdown lint (deferred stub), .json → JSON parse.",
            json!({
                "type": "object",
                "required": ["task_id", "artifact_path"],
                "properties": {
                    "task_id": {"type": "string"},
                    "artifact_path": {"type": "string"}
                }
            }),
        ),
        tool(
            "read_artifact_diff",
            "Git diff of the artifact against a base ref (defaults to HEAD). Runs inside the per-task workdir. Returns the same {exit_code, stdout_tail, …} shape as run_tests.",
            json!({
                "type": "object",
                "required": ["task_id", "artifact_path"],
                "properties": {
                    "task_id": {"type": "string"},
                    "artifact_path": {"type": "string"},
                    "base_ref": {"type": "string"}
                }
            }),
        ),
    ]);
    json!({ "tools": tools })
}

/// Translate MCP `tools/call` (params `{name, arguments}`) into the internal
/// `mcp_call` envelope and route it through the world loop. The loop calls
/// `mcp_dispatch::dispatch`, which returns either `mcp_reply` or `mcp_error`.
/// Both shapes are wrapped as MCP `content` (per MCP 2025-03-26 §
/// "Server-side errors are reported with isError=true, not JSON-RPC error").
async fn handle_tools_call(
    s: &Arc<AppState>,
    agent_id: &str,
    params: Value,
) -> Value {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return tool_error_content("missing tool name");
    }
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

    // The HTTP handler doesn't generate a corr_id beyond the JSON-RPC `id`
    // because `Cmd::HandleWorkerMsg` already gives us a per-call oneshot —
    // we don't multiplex multiple in-flight calls over one socket like the
    // WS path does. Still pass a corr_id so log lines round-trip cleanly.
    let corr_id = format!("http-{}", uuid::Uuid::new_v4());
    let envelope = json!({
        "type": "mcp_call",
        "v": 1,
        "tool": name,
        "args": arguments,
        "corr_id": corr_id,
    });

    let (tx, rx) = oneshot::channel();
    if s.handle
        .tx
        .send(Cmd::HandleWorkerMsg {
            agent_id: agent_id.to_string(),
            msg: envelope,
            reply: tx,
        })
        .await
        .is_err()
    {
        return tool_error_content("world loop disconnected");
    }
    let reply = match rx.await {
        Ok(v) => v,
        Err(_) => return tool_error_content("world loop dropped reply channel"),
    };

    // Unwrap `mcp_reply` / `mcp_error` into MCP `content` shape. Tool-level
    // failures (e.g. `bad_artifact_path`) become `isError:true` per spec; the
    // text payload stays JSON so callers can parse structured failure info.
    match reply.get("type").and_then(|v| v.as_str()) {
        Some("mcp_reply") => {
            let result = reply.get("result").cloned().unwrap_or(Value::Null);
            json!({
                "content": [{
                    "type": "text",
                    "text": result.to_string(),
                }],
                "isError": false,
            })
        }
        Some("mcp_error") => {
            let code = reply.get("code").and_then(|v| v.as_str()).unwrap_or("error");
            let message = reply
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            json!({
                "content": [{
                    "type": "text",
                    "text": json!({"code": code, "message": message}).to_string(),
                }],
                "isError": true,
            })
        }
        _ => tool_error_content("dispatch returned unknown reply shape"),
    }
}

fn tool_error_content(msg: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": msg }],
        "isError": true,
    })
}
