//! M9.10 A1' — MCP-over-HTTP at the world.
//!
//! Asserts the four contract points spec'd in
//! `docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md` § A1':
//!   - `initialize` returns a protocolVersion + capabilities + serverInfo handshake.
//!   - `tools/list` returns the 16 cliptown tools by name.
//!   - `tools/call task_done` routes through `mcp_dispatch::dispatch` and the
//!     SQL side effect lands (`tasks.status` flips to `awaiting_review`).
//!   - A missing/bad Bearer token returns 401 before the dispatch runs.
//!
//! Auth model: `Authorization: Bearer <agent_id>:<secret>` — single token
//! encodes both halves so adapter `mcp.json` keeps one `headers` field.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use cliptown_world::{
    agent_supervisor::{AgentSupervisor, SupervisorConfig},
    http, loop_,
    state::{AvatarView, WorldView},
    storage,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

/// Boot a fully-wired world (pool + loop + axum app) and seed the e1
/// engineer + T1 subtask used by the task_done happy-path test. Returns
/// the axum `Router` ready for `oneshot`. The tempdir is forgotten so it
/// outlives the router; acceptable in CI per the ws_auth fixture's note.
async fn boot() -> axum::Router {
    let dir = tempfile::tempdir().unwrap();
    let pool = storage::open(dir.path().join("test.db").to_str().unwrap())
        .await
        .unwrap();

    // Schema bootstrap. seed_if_empty fills the default town + rooms;
    // mcp_handlers.rs' fixture does the same.
    cliptown_world::seed::seed_if_empty(&pool).await.unwrap();

    // Two-startup seed mirrors `crates/world/tests/mcp_handlers.rs::fixture`
    // so a task_done call against T1 exercises the same path as the unit
    // tests but through the HTTP transport.
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'alpha', 'g', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, manager_id, status) \
         VALUES ('m1', 's1', 'M1', 'founder', 'claude_code', 'm', '{}', 'suite_1', NULL, 'idle')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, manager_id, status) \
         VALUES ('e1', 's1', 'E1', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'm1', 'idle')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T0', 's1', 'root', 'd', 'in_progress', 'm1', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 'T0', 'sub', 'd', 'in_progress', 'e1', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Seed the in-memory world with e1's avatar so `mcp_dispatch::dispatch`'s
    // `world.avatars.get(agent_id)` lookup succeeds. Production gets this via
    // the loop's `InsertAvatars` cmd; tests can short-circuit by handing it
    // to `loop_::spawn` directly.
    let mut initial = WorldView::default();
    initial.avatars.insert(
        "e1".into(),
        AvatarView {
            agent_id: "e1".into(),
            startup_id: "s1".into(),
            role: "engineer".into(),
            backend: "claude_code".into(),
            current_pos: (4, 3),
            target_pos: None,
            room_id: "suite_1".into(),
            status: "idle".into(),
            last_seen_at: None,
            health: cliptown_world::health::Health::Offline,
        },
    );

    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(64);
    let handle = loop_::spawn(initial, pool.clone(), event_tx.clone());
    let catalog = std::sync::Arc::new(tokio::sync::RwLock::new(
        std::collections::HashMap::new(),
    ));
    let supervisor = Arc::new(AgentSupervisor::new(
        SupervisorConfig::default(),
        pool.clone(),
        event_tx.clone(),
    ));
    let app = http::router(http::AppState {
        pool,
        handle,
        catalog,
        supervisor,
        max_review_rounds: 3,
    });
    std::mem::forget(dir);
    app
}

fn rpc_request(method: &str, params: Value) -> Body {
    Body::from(
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        })
        .to_string(),
    )
}

async fn post_mcp(app: axum::Router, token: Option<&str>, body: Body) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = app.oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let v: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, v)
}

#[tokio::test]
async fn initialize_returns_handshake() {
    let app = boot().await;
    let (status, body) = post_mcp(
        app,
        Some("e1:dev-secret"),
        rpc_request(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name":"test","version":"0"},
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 1);
    let result = &body["result"];
    assert!(
        result["protocolVersion"].is_string(),
        "missing protocolVersion: {body}"
    );
    assert!(
        result["capabilities"]["tools"].is_object(),
        "missing capabilities.tools: {body}"
    );
    assert_eq!(result["serverInfo"]["name"], "cliptown-world");
}

#[tokio::test]
async fn tools_list_returns_all_24_cliptown_tools() {
    let app = boot().await;
    let (status, body) = post_mcp(
        app,
        Some("e1:dev-secret"),
        rpc_request("tools/list", json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let tools = body["result"]["tools"].as_array().expect("tools array");
    let names: std::collections::BTreeSet<String> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap_or("").to_string())
        .collect();
    let expected: std::collections::BTreeSet<String> = [
        "move_intent",
        "speak",
        "task_done",
        "task_failed",
        "subtask_create",
        "task_accept",
        "task_request_changes",
        "accept_proposal",
        "reject_proposal",
        "hypothesis_state",
        "test_record",
        "hypothesis_resolve",
        "verify",
        "ask_peer",
        "observe_world",
        "read_artifact",
        "skill_upsert",
        "skill_list",
        "skill_attach",
        "skill_detach",
        "skill_delete",
        "task_set_preference",
        "skill_file_upsert",
        "skill_file_delete",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(names, expected, "tools/list must enumerate all 24 names");
    // Each tool must carry at least a minimal inputSchema — MCP spec requires it.
    for t in tools {
        assert!(
            t["inputSchema"]["type"].is_string(),
            "tool {} missing inputSchema.type",
            t["name"]
        );
    }
}

#[tokio::test]
async fn tools_call_task_done_routes_through_dispatch() {
    let app = boot().await;
    // Pool handle for the post-call SQL assert. boot() forgets its tempdir
    // so the pool stays valid through the test.
    let app_for_call = app.clone();
    let (status, body) = post_mcp(
        app_for_call,
        Some("e1:dev-secret"),
        rpc_request(
            "tools/call",
            json!({
                "name": "task_done",
                "arguments": {
                    "task_id": "T1",
                    "artifact_path": "workspaces/s1/artifacts/T1.md",
                }
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert!(
        body["result"]["isError"].as_bool() != Some(true),
        "tools/call task_done unexpectedly errored: {body}"
    );
    // MCP `tools/call` result shape: { content: [{type:"text", text:"..."}], isError? }
    let content = body["result"]["content"]
        .as_array()
        .expect("result.content array");
    let text = content[0]["text"].as_str().expect("content[0].text string");
    let payload: Value = serde_json::from_str(text).expect("content text is valid JSON");
    assert_eq!(payload["task_id"], "T1");
    assert_eq!(payload["new_status"], "awaiting_review");
}

#[tokio::test]
async fn bad_token_returns_401() {
    let app = boot().await;
    let (status, _body) = post_mcp(
        app,
        Some("e1:WRONG"),
        rpc_request("tools/list", json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn missing_token_returns_401() {
    let app = boot().await;
    let (status, _body) = post_mcp(app, None, rpc_request("tools/list", json!({}))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// MCP streamable-HTTP transport: notifications must get HTTP 202 Accepted
/// with EMPTY body. rmcp 0.6+ clients (used by `codex exec --json`) reject
/// any JSON-RPC-shaped response — including `{}` — to a notification with
/// `Deserialize error: data did not match any variant of untagged enum
/// JsonRpcMessage`. Regression guard for that compatibility fix.
#[tokio::test]
async fn notifications_initialized_returns_202_empty_body() {
    let app = boot().await;
    let body = Body::from(
        json!({
            "jsonrpc": "2.0",
            // No `id` field — this is a notification, not a request.
            "method": "notifications/initialized",
            "params": {},
        })
        .to_string(),
    );
    let req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", "Bearer e1:dev-secret")
        .body(body)
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    assert_eq!(bytes.len(), 0, "notifications must return empty body");
}
