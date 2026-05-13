//! M5.3 end-to-end integration test: operator directive →
//! founder subtask → engineer assignment.
//!
//! Drives `cmd_console::dispatch`, `mcp_dispatch::dispatch`, and
//! `scheduler::tick` directly to exercise the full chain at the
//! world-server level (no WS/CLI). Phase 0 scope per plan §5.3.

use cliptown_world::{
    cmd_console, mcp_dispatch,
    move_sys::{self, PathStore},
    path::RoomGraph,
    scheduler,
    seed::{self, TownLayout},
    state::{AvatarView, WorldView},
    storage,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Standard fixture for the M5.3 chain test:
/// - seeded default town
/// - startup `s1`
/// - founder `founder1` (root, no manager_id)
/// - engineer `eng1` (manager_id = founder1)
/// - designer `des1` (manager_id = founder1)
/// - root parent task assigned to founder1, in_progress (so founder is the
///   parent's assignee → manager of any subtask under it).
async fn fixture() -> (
    sqlx::SqlitePool,
    TownLayout,
    RoomGraph,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("e2e-directive-chain.db");
    let pool = storage::open(p.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'alpha', 'goal', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())",
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
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status, manager_id) \
         VALUES ('des1', 's1', 'D', 'designer', 'claude_code', 'm', '{}', 'suite_1', 'idle', 'founder1')",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Root parent task assigned to the founder. subtask_create's manager check
    // (mcp_dispatch::handle_subtask_create) reads `parent.assignee_agent_id`
    // and treats the caller as manager when it equals their agent_id. So this
    // is what makes the founder a manager of subtasks under this parent.
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('parent-task', 's1', NULL, 'parent', 'desc', 'in_progress', 'founder1', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();

    let layout = TownLayout::default_town();
    let graph = move_sys::graph_from_layout(&layout);
    (pool, layout, graph, dir)
}

fn av(id: &str, role: &str, room: &str) -> AvatarView {
    AvatarView {
        agent_id: id.to_string(),
        startup_id: "s1".to_string(),
        role: role.to_string(),
        backend: "claude_code".to_string(),
        current_pos: (3, 3),
        target_pos: None,
        room_id: room.to_string(),
        status: "idle".to_string(),
        last_seen_at: None,
        health: cliptown_world::health::Health::Offline,
    }
}

#[tokio::test]
async fn directive_to_founder_then_subtask_then_assigned() {
    let (pool, layout, graph, _dir) = fixture().await;

    let mut w = WorldView::default();
    w.avatars
        .insert("founder1".to_string(), av("founder1", "founder", "suite_1"));
    w.avatars
        .insert("eng1".to_string(), av("eng1", "engineer", "suite_1"));
    w.avatars
        .insert("des1".to_string(), av("des1", "designer", "suite_1"));

    let mut paths: PathStore = HashMap::new();
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(64);

    // Mock workers: tx into out_bus, hold rx ends to assert delivery.
    let mut out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (founder_tx, mut founder_rx) = mpsc::channel(8);
    let (eng_tx, mut eng_rx) = mpsc::channel(8);
    let (des_tx, _des_rx) = mpsc::channel(8);
    out_bus.insert("founder1".to_string(), founder_tx);
    out_bus.insert("eng1".to_string(), eng_tx);
    out_bus.insert("des1".to_string(), des_tx);

    // ── Step 1: operator sends OperatorDirective via cmd_console.
    let r = cmd_console::dispatch(
        &mut w,
        &pool,
        &out_bus,
        &event_tx, &cliptown_world::auth::OperatorIdentity::admin_for_tests(), json!({
            "type": "operator_directive",
            "v": 1,
            "to_agent_id": "founder1",
            "body": "build a spec.md describing how to deploy"
        }),
    )
    .await;
    assert_eq!(r["type"], "ok", "operator_directive should succeed: {r}");

    // Founder's worker receives the directive on its out_bus.
    let directive = founder_rx
        .try_recv()
        .expect("founder should have received directive");
    assert_eq!(directive["type"], "directive");
    assert_eq!(directive["from_agent_id"], "operator");
    assert_eq!(directive["body"], "build a spec.md describing how to deploy");

    // SQL row also exists (same-transaction invariant from M1.12).
    let row: (String, String) = sqlx::query_as(
        "SELECT author_id, kind FROM messages WHERE id = ?",
    )
    .bind(directive["message_id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "operator");
    assert_eq!(row.1, "directive");

    // Verify the Directive broadcast frame reached the operator console.
    match event_rx.try_recv() {
        Ok(cliptown_world::protocol::ConsoleOutbound::Directive {
            author_id, to_agent_id, body, in_response_to_task, ..
        }) => {
            assert_eq!(author_id, "operator");
            assert_eq!(to_agent_id, "founder1");
            assert!(body.contains("spec") || body.contains("build"), "body should be the directive: {body}");
            assert_eq!(in_response_to_task, None);
        }
        other => panic!("expected Directive frame, got {:?}", other),
    }

    // ── Step 2: founder calls subtask_create assigning the engineer.
    // Founder is the parent task's assignee → manager → status goes straight
    // to `queued` (not `proposed`).
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
            "type": "mcp_call",
            "v": 1,
            "tool": "subtask_create",
            "corr_id": "c1",
            "args": {
                "parent_id": "parent-task",
                "title": "Write spec.md",
                "description": "Describe deployment steps.",
                "assignee_agent_id": "eng1",
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
    let status = r["result"]["status"].as_str().expect("status in reply");
    assert_eq!(status, "queued", "founder is manager → queued, not proposed");

    // ── Step 3: scheduler tick → task_assigned to engineer's out_bus.
    let n = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool, None).await;
    assert_eq!(n, 1, "scheduler should dispatch the queued task");

    let assigned = eng_rx
        .try_recv()
        .expect("engineer should have received task_assigned");
    assert_eq!(assigned["type"], "task_assigned");
    assert_eq!(assigned["task_id"], task_id);
    assert_eq!(assigned["title"], "Write spec.md");

    // SQL state: task is in_progress, assignee_agent_id is eng1.
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT status, assignee_agent_id FROM tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "in_progress");
    assert_eq!(row.1.as_deref(), Some("eng1"));

    // Engineer avatar is flagged busy in-memory (scheduler invariant).
    assert_eq!(w.avatars["eng1"].status, "working");

    let mut found = Vec::new();
    loop {
        match event_rx.try_recv() {
            Ok(frame) => found.push(frame),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
        }
    }
    assert!(found.is_empty(), "expected no console broadcasts, found {}: {:?}", found.len(), found);
}
