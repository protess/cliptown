//! M5.6 — Full review cycle E2E + max_review_rounds escalation.
//!
//! Two scenarios:
//!   1. `round1_request_changes_then_round2_accept`: founder bounces T1 once
//!      with feedback, engineer revises, founder accepts in round 2. Asserts
//!      the directive carries the feedback, `review_round = 1`, status `done`,
//!      and the audit trail records both the request_changes and the accept.
//!   2. `max_review_rounds_escalates`: with `review_round` pre-set to the cap,
//!      a fourth `task_request_changes` auto-escalates the task to `escalated`,
//!      writes a `task_escalated` row to `system_events`, and surfaces the
//!      escalation reason in both the audit trail and the reply payload.
//!
//! The escalation path lives in `mcp_dispatch::handle_task_request_changes`
//! and routes through `task_sm::next(Escalate)` so the SM stays the single
//! source of truth on legal transitions.

mod common;

use cliptown_world::{
    mcp_dispatch,
    move_sys::{self, PathStore},
    path::RoomGraph,
    protocol::ConsoleOutbound,
    seed::{self, TownLayout},
    state::{AvatarView, WorldView},
    storage,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::{broadcast, mpsc};

async fn fixture() -> (sqlx::SqlitePool, TownLayout, RoomGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("e2e-review-cycle.db");
    let pool = storage::open(p.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'alpha', 'goal', 10.0, 'town_default', 'workspaces/s1', 'active', unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status, manager_id) \
         VALUES ('founder1', 's1', 'F', 'founder', 'claude_code', 'm', '{}', 'suite_1', 'idle', NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status, manager_id) \
         VALUES ('eng1', 's1', 'E', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle', 'founder1')",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Parent task assigned to the founder so founder is T1's manager (parent's
    // assignee). Mirrors the e2e_manager_accept fixture.
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('parent', 's1', NULL, 'parent', 'desc', 'in_progress', 'founder1', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();
    // T1 — engineer's subtask, sitting in awaiting_review (engineer just emitted task_done).
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 'parent', 'Write spec', 'desc', 'awaiting_review', 'eng1', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();

    let layout = TownLayout::default_town();
    let graph = move_sys::graph_from_layout(&layout);
    (pool, layout, graph, dir)
}

fn av(id: &str, role: &str) -> AvatarView {
    AvatarView {
        agent_id: id.to_string(),
        startup_id: "s1".to_string(),
        role: role.to_string(),
        backend: "claude_code".to_string(),
        current_pos: (3, 3),
        target_pos: None,
        room_id: "suite_1".to_string(),
        status: "idle".to_string(),
    }
}

fn make_event_tx() -> (broadcast::Sender<ConsoleOutbound>, broadcast::Receiver<ConsoleOutbound>) {
    broadcast::channel(64)
}

#[tokio::test]
async fn round1_request_changes_then_round2_accept() {
    let (pool, layout, graph, _dir) = fixture().await;
    let mut w = WorldView::default();
    w.avatars.insert("founder1".to_string(), av("founder1", "founder"));
    w.avatars.insert("eng1".to_string(), av("eng1", "engineer"));

    let mut paths: PathStore = HashMap::new();
    let mut out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (eng_tx, mut eng_rx) = mpsc::channel(8);
    out_bus.insert("eng1".to_string(), eng_tx);
    let (event_tx, mut event_rx) = make_event_tx();

    // Materialize a v1 artifact so read_artifact would work, even though the
    // spec doesn't require the founder to have read it before requesting
    // changes — keeps the fixture realistic.
    let artifacts = std::path::PathBuf::from("workspaces/s1/artifacts");
    tokio::fs::create_dir_all(&artifacts).await.unwrap();
    let artifact_path = artifacts.join("T1.md");
    tokio::fs::write(&artifact_path, "# Spec v1\n\n## Goal\n\nDeploy.\n")
        .await
        .unwrap();

    // Round 1: founder calls task_request_changes with feedback.
    let r = mcp_dispatch::dispatch(
        &mut w,
        &mut paths,
        &layout,
        &graph,
        &out_bus,
        &pool,
        &event_tx,
        "founder1",
        json!({
            "type": "mcp_call", "v": 1, "tool": "task_request_changes", "corr_id": "c1",
            "args": { "task_id": "T1", "feedback": "needs more detail", "in_response_to_round": 0 }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "request_changes round 1: {r}");
    assert_eq!(r["result"]["new_status"], "changes_requested");

    // Engineer received the directive carrying the feedback.
    let directive = eng_rx
        .try_recv()
        .expect("engineer should receive feedback directive");
    assert_eq!(directive["type"], "directive");
    assert_eq!(directive["from_agent_id"], "founder1");
    assert!(
        directive["body"]
            .as_str()
            .unwrap_or("")
            .contains("needs more detail"),
        "directive body should carry feedback: {directive}"
    );
    assert_eq!(directive["in_response_to_task"], "T1");

    let row: (String, i64) =
        sqlx::query_as("SELECT status, review_round FROM tasks WHERE id = 'T1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "changes_requested");
    assert_eq!(row.1, 1);

    // Engineer revises the artifact and re-emits task_done. The state machine
    // permits ChangesRequested -> AwaitingReview via TaskDoneMcp.
    tokio::fs::write(
        &artifact_path,
        "# Spec v2\n\n## Goal\n\nDeploy with more detail.\n## Plan\n\nStep-by-step.\n",
    )
    .await
    .unwrap();

    let r = mcp_dispatch::dispatch(
        &mut w,
        &mut paths,
        &layout,
        &graph,
        &out_bus,
        &pool,
        &event_tx,
        "eng1",
        json!({
            "type": "mcp_call", "v": 1, "tool": "task_done", "corr_id": "c2",
            "args": {
                "task_id": "T1",
                "artifact_path": "workspaces/s1/artifacts/T1.md",
            }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "task_done revisited: {r}");
    assert_eq!(r["result"]["new_status"], "awaiting_review");

    // Round 2: founder accepts.
    let r = mcp_dispatch::dispatch(
        &mut w,
        &mut paths,
        &layout,
        &graph,
        &out_bus,
        &pool,
        &event_tx,
        "founder1",
        json!({
            "type": "mcp_call", "v": 1, "tool": "task_accept", "corr_id": "c3",
            "args": { "task_id": "T1" }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "task_accept round 2: {r}");
    assert_eq!(r["result"]["new_status"], "done");

    let row: (String, i64, String) = sqlx::query_as(
        "SELECT status, review_round, audit_trail FROM tasks WHERE id = 'T1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "done");
    assert_eq!(
        row.1, 1,
        "review_round should still be 1 after accept (only request_changes increments)"
    );
    let trail = row.2;
    assert!(
        trail.contains("task_request_changes"),
        "audit_trail should record round 1 request_changes: {trail}"
    );
    assert!(
        trail.contains("task_done"),
        "audit_trail should record round 2 task_done: {trail}"
    );
    assert!(
        trail.contains("task_accept"),
        "audit_trail should record round 2 task_accept: {trail}"
    );

    // Cleanup so the workspaces/ dir doesn't leak between test runs.
    let _ = tokio::fs::remove_file(&artifact_path).await;
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn max_review_rounds_escalates() {
    let (pool, layout, graph, _dir) = fixture().await;
    let mut w = WorldView::default();
    w.avatars.insert("founder1".to_string(), av("founder1", "founder"));
    w.avatars.insert("eng1".to_string(), av("eng1", "engineer"));

    let mut paths: PathStore = HashMap::new();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (event_tx, mut event_rx) = make_event_tx();

    // Pre-set review_round = 3 (the cap) with the task back in awaiting_review.
    // The next request_changes should auto-escalate instead of bouncing.
    sqlx::query("UPDATE tasks SET review_round = 3, status = 'awaiting_review' WHERE id = 'T1'")
        .execute(&pool)
        .await
        .unwrap();

    let r = mcp_dispatch::dispatch(
        &mut w,
        &mut paths,
        &layout,
        &graph,
        &out_bus,
        &pool,
        &event_tx,
        "founder1",
        json!({
            "type": "mcp_call", "v": 1, "tool": "task_request_changes", "corr_id": "c1",
            "args": { "task_id": "T1", "feedback": "still wrong", "in_response_to_round": 3 }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "request_changes at max: {r}");
    assert_eq!(r["result"]["new_status"], "escalated");
    assert_eq!(r["result"]["status"], "escalated");
    assert_eq!(r["result"]["reason"], "max_review_rounds_exceeded");

    // SQL state: status escalated, review_round preserved (we didn't bump
    // because the round wasn't actually consumed).
    let row: (String, i64) =
        sqlx::query_as("SELECT status, review_round FROM tasks WHERE id = 'T1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "escalated");
    assert_eq!(row.1, 3);

    // system_events row written at severity=alert.
    let evt: (String, String) = sqlx::query_as(
        "SELECT severity, payload FROM system_events WHERE kind = 'task_escalated'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(evt.0, "alert");
    assert!(
        evt.1.contains("\"task_id\":\"T1\""),
        "payload should reference T1: {}",
        evt.1
    );

    // Audit trail records the escalation with the trigger reason.
    let trail: (String,) = sqlx::query_as("SELECT audit_trail FROM tasks WHERE id = 'T1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        trail.0.contains("max_review_rounds_exceeded"),
        "audit_trail should mention escalation reason: {}",
        trail.0
    );
    assert!(
        trail.0.contains("escalated"),
        "audit_trail should mention escalated kind: {}",
        trail.0
    );
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}
