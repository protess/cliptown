//! M5.5 — Manager accept loop closes invariants 2–5.
//!
//! Drives the founder-side review path through `mcp_dispatch::dispatch`:
//!   1. Founder calls `read_artifact` and reads the engineer's spec.
//!   2. Founder calls `task_accept` and the world transitions T1 to `done`.
//!   3. The audit_trail picks up a `task_accept` entry.
//!   4. A non-manager (the assignee themselves) is denied with `no_permission`,
//!      and the task does not transition.
//!
//! This is a verification test of M2.3 logic in an end-to-end shape — no new
//! source-side logic is added. Pairs with `e2e_engineer_artifact.rs` (M5.4)
//! to cover the full subtask lifecycle for Phase 0.

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
    let p = dir.path().join("e2e-manager-accept.db");
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
    // Parent task assigned to the founder (so founder is the manager of T1
    // by virtue of being the parent's assignee).
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('parent', 's1', NULL, 'parent', 'desc', 'in_progress', 'founder1', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();
    // T1 — engineer's task, already in awaiting_review (M5.4 outcome).
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
        last_seen_at: None,
        health: cliptown_world::health::Health::Offline,
    }
}

fn make_event_tx() -> (broadcast::Sender<ConsoleOutbound>, broadcast::Receiver<ConsoleOutbound>) {
    broadcast::channel(64)
}

#[tokio::test]
async fn founder_reads_artifact_then_accepts() {
    let (pool, layout, graph, _dir) = fixture().await;
    let mut w = WorldView::default();
    w.avatars.insert("founder1".to_string(), av("founder1", "founder"));
    w.avatars.insert("eng1".to_string(), av("eng1", "engineer"));

    let mut paths: PathStore = HashMap::new();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (event_tx, mut event_rx) = make_event_tx();

    // Materialize the artifact at the canonical location so read_artifact
    // can find it. sandbox::resolve canonicalizes the workspace root, so the
    // directory must exist on disk before the call.
    let artifacts = std::path::PathBuf::from("workspaces/s1/artifacts");
    tokio::fs::create_dir_all(&artifacts).await.unwrap();
    let artifact_path = artifacts.join("T1.md");
    tokio::fs::write(&artifact_path, "# Spec\n\n## Goal\n\nDeploy.\n")
        .await
        .unwrap();

    // 1. read_artifact — founder reviews the engineer's output.
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
            "type": "mcp_call", "v": 1, "tool": "read_artifact", "corr_id": "c1",
            "args": { "path": "artifacts/T1.md" }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "read_artifact: {r}");
    let content = r["result"]["content"].as_str().unwrap_or("");
    assert!(content.contains("Goal"), "expected 'Goal' in artifact: {content}");

    // 2. task_accept — founder closes out the subtask.
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
            "type": "mcp_call", "v": 1, "tool": "task_accept", "corr_id": "c2",
            "args": { "task_id": "T1" }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "task_accept: {r}");
    assert_eq!(r["result"]["new_status"], "done");

    // SQL state — task is done and audit_trail mentions task_accept.
    let row: (String, String) =
        sqlx::query_as("SELECT status, audit_trail FROM tasks WHERE id = 'T1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "done");
    assert!(
        row.1.contains("task_accept"),
        "audit_trail should contain task_accept: {}",
        row.1
    );

    // Cleanup so the workspaces/ dir doesn't leak between test runs.
    let _ = tokio::fs::remove_file(&artifact_path).await;
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn non_manager_cannot_accept() {
    let (pool, layout, graph, _dir) = fixture().await;
    let mut w = WorldView::default();
    w.avatars.insert("eng1".to_string(), av("eng1", "engineer"));

    let mut paths: PathStore = HashMap::new();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (event_tx, mut event_rx) = make_event_tx();

    // Engineer is the assignee of T1, NOT the manager. Manager is the
    // founder (parent's assignee). Self-acceptance must be denied.
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
            "type": "mcp_call", "v": 1, "tool": "task_accept", "corr_id": "c1",
            "args": { "task_id": "T1" }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_error", "non-manager should be denied: {r}");
    assert_eq!(r["code"], "no_permission", "{r}");

    // Task remains in awaiting_review — no transition fired.
    let row: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id = 'T1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        row.0, "awaiting_review",
        "task should not have transitioned"
    );
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}
