//! M6.2 end-to-end integration test: engineer-proposed subtask →
//! operator drag-drop accept (Proposed → Queued) → scheduler dispatch.
//!
//! Phase 0 keeps testing server-side per plan §6.2 — Playwright is
//! out of scope; the chain is driven directly through `mcp_dispatch`,
//! `cmd_console::dispatch`, and `scheduler::tick`. The "drag-drop"
//! step is modeled as the `OperatorAcceptProposal` console message
//! that the future /console UI will emit.
//!
//! Asserts:
//!   1. Non-manager engineer's `subtask_create` lands the task in
//!      `proposed` and fans `subtask_proposed` to the founder via
//!      out_bus (manager === parent task assignee).
//!   2. Operator's `OperatorAcceptProposal` flips Proposed → Queued
//!      and writes an audit entry with `actor=operator`.
//!   3. Scheduler tick picks up the queued task → in_progress and
//!      fans `task_assigned` to the engineer's worker; audit row
//!      with `actor=scheduler` is appended.
//!   4. The final `audit_trail` contains both `accept_proposal` and
//!      `task_assigned` events.
//!
//! A second scenario covers the reject path: operator rejects, task
//! goes Proposed → Failed, audit captures the reason and actor.

mod common;

use cliptown_world::{
    cmd_console, mcp_dispatch,
    move_sys::{self, PathStore},
    path::RoomGraph,
    protocol::ConsoleOutbound,
    scheduler,
    seed::{self, TownLayout},
    state::{AvatarView, WorldView},
    storage,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::{broadcast, mpsc};

/// Fixture: seeded default town, startup `s1`, founder `founder1`
/// (root, no manager_id), engineer `eng1` (manager_id = founder1),
/// and a parent task `parent` assigned to founder1 in_progress so
/// the founder is treated as the manager for any subtask under it
/// (subtask_create checks `parent.assignee_agent_id`).
async fn fixture() -> (sqlx::SqlitePool, TownLayout, RoomGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("e2e-proposal-flow.db");
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
    // Parent task assigned to founder. subtask_create's manager check
    // (mcp_dispatch::handle_subtask_create) reads `parent.assignee_agent_id`
    // and treats the caller as a non-manager when it differs — which is the
    // path we want here (the engineer caller).
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('parent', 's1', NULL, 'parent', 'desc', 'in_progress', 'founder1', unixepoch(), unixepoch())",
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
async fn proposal_to_queued_to_assigned() {
    let (pool, layout, graph, _dir) = fixture().await;
    let mut w = WorldView::default();
    w.avatars
        .insert("founder1".to_string(), av("founder1", "founder"));
    w.avatars.insert("eng1".to_string(), av("eng1", "engineer"));

    let mut paths: PathStore = HashMap::new();
    let mut out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (founder_tx, mut founder_rx) = mpsc::channel(8);
    let (eng_tx, mut eng_rx) = mpsc::channel(8);
    out_bus.insert("founder1".to_string(), founder_tx);
    out_bus.insert("eng1".to_string(), eng_tx);
    let (event_tx, mut event_rx) = make_event_tx();

    // ── Step 1: engineer (non-manager) calls subtask_create.
    // Non-managers can't pre-pick the assignee (mcp_dispatch nulls it
    // out); the task lands in `proposed`.
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
            "type": "mcp_call",
            "v": 1,
            "tool": "subtask_create",
            "corr_id": "c1",
            "args": {
                "parent_id": "parent",
                "title": "From engineer",
                "description": "engineer-proposed work",
                "assignee_agent_id": null,
                "required_room": null,
            }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "subtask_create should succeed: {r}");
    let task_id = r["result"]["task_id"]
        .as_str()
        .expect("task_id in reply")
        .to_string();
    assert_eq!(r["result"]["status"], "proposed");

    // Founder receives `subtask_proposed` event.
    let proposed = founder_rx
        .try_recv()
        .expect("founder should see subtask_proposed");
    assert_eq!(proposed["type"], "subtask_proposed");
    // The event carries `proposed_task_id` (mcp_dispatch::handle_subtask_create).
    assert_eq!(proposed["proposed_task_id"], task_id);
    assert_eq!(proposed["proposer_agent_id"], "eng1");
    assert_eq!(proposed["parent_id"], "parent");

    // ── Step 2: operator drags proposed → queued via OperatorAcceptProposal.
    let r = cmd_console::dispatch(
        &mut w,
        &pool,
        &out_bus,
        &event_tx,
        json!({
            "type": "operator_accept_proposal",
            "v": 1,
            "task_id": task_id,
            "assignee_agent_id": "eng1",
            "required_room": null,
        }),
    )
    .await;
    assert_eq!(r["type"], "ok", "operator_accept_proposal: {r}");

    let row: (String, Option<String>, String) = sqlx::query_as(
        "SELECT status, assignee_agent_id, audit_trail FROM tasks WHERE id = ?",
    )
    .bind(&task_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "queued");
    assert_eq!(row.1.as_deref(), Some("eng1"));
    let audit = row.2;
    assert!(
        audit.contains("accept_proposal"),
        "audit_trail missing accept_proposal: {audit}"
    );
    assert!(
        audit.contains("\"actor\":\"operator\""),
        "audit_trail missing actor=operator: {audit}"
    );

    // ── Step 3: scheduler tick → in_progress, fans task_assigned to engineer.
    let n = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool).await;
    assert_eq!(n, 1, "scheduler should dispatch the queued task");

    let assigned = eng_rx
        .try_recv()
        .expect("engineer should see task_assigned");
    assert_eq!(assigned["type"], "task_assigned");
    assert_eq!(assigned["task_id"], task_id);

    // ── Step 4: audit_trail carries both accept_proposal and task_assigned.
    let row: (String,) = sqlx::query_as("SELECT audit_trail FROM tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        row.0.contains("accept_proposal"),
        "audit_trail missing accept_proposal: {}",
        row.0
    );
    assert!(
        row.0.contains("task_assigned"),
        "audit_trail missing task_assigned: {}",
        row.0
    );
    assert!(
        row.0.contains("\"actor\":\"scheduler\""),
        "audit_trail missing actor=scheduler: {}",
        row.0
    );
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn proposal_rejected_by_operator_audit_trail() {
    let (pool, layout, graph, _dir) = fixture().await;
    let mut w = WorldView::default();
    w.avatars.insert("eng1".to_string(), av("eng1", "engineer"));
    w.avatars
        .insert("founder1".to_string(), av("founder1", "founder"));
    let mut paths: PathStore = HashMap::new();
    // No out_bus entries — the founder's subtask_proposed delivery is best-effort
    // (try_send) and absent recipients are a no-op; we don't assert on it here.
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (event_tx, mut event_rx) = make_event_tx();

    // Engineer proposes.
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
            "type": "mcp_call",
            "v": 1,
            "tool": "subtask_create",
            "corr_id": "c1",
            "args": {
                "parent_id": "parent",
                "title": "Reject me",
                "description": "test",
                "assignee_agent_id": null,
                "required_room": null,
            }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "subtask_create should succeed: {r}");
    let task_id = r["result"]["task_id"]
        .as_str()
        .expect("task_id in reply")
        .to_string();
    assert_eq!(r["result"]["status"], "proposed");

    // Operator rejects.
    let r = cmd_console::dispatch(
        &mut w,
        &pool,
        &out_bus,
        &event_tx,
        json!({
            "type": "operator_reject_proposal",
            "v": 1,
            "task_id": task_id,
            "reason": "out of scope",
        }),
    )
    .await;
    assert_eq!(r["type"], "ok", "operator_reject_proposal: {r}");

    let row: (String, String) = sqlx::query_as(
        "SELECT status, audit_trail FROM tasks WHERE id = ?",
    )
    .bind(&task_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "failed");
    assert!(
        row.1.contains("reject_proposal"),
        "audit_trail missing reject_proposal: {}",
        row.1
    );
    assert!(
        row.1.contains("out of scope"),
        "audit_trail missing reason: {}",
        row.1
    );
    assert!(
        row.1.contains("\"actor\":\"operator\""),
        "audit_trail missing actor=operator: {}",
        row.1
    );
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}
