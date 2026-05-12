//! M5.4 — Engineer fixture writes artifact with epistemic discipline.
//!
//! Drives the M2.4 `engineer_writes_spec.jsonl` fixture sequence through
//! `mcp_dispatch::dispatch` directly (no WS workers) and asserts:
//!   1. The canonical artifact lands at `workspaces/<sid>/artifacts/<tid>.md`.
//!   2. The task transitions to `awaiting_review`.
//!   3. All five epistemic_log entries (state/test/resolve) are present.
//!   4. The audit_trail has a `task_done` entry.
//!   5. A non-canonical artifact path is rejected with `bad_artifact_path`.
//!
//! The fixture's `verify` step uses `params.substring` in the spec, but the
//! world handler's `read_assert` reads `params.contains`. Phase 0 keeps the
//! key on the world side; M3+ wires the worker translation. The test reflects
//! the world contract.

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
    let p = dir.path().join("e2e-engineer-artifact.db");
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
    // Parent task assigned to founder1, T1 (engineer's task) is its subtask in
    // in_progress. Matches the M5.3 chain shape: founder is parent's assignee
    // → manager of T1 → will receive the subtask_done fanout.
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T0', 's1', NULL, 'parent', 'desc', 'in_progress', 'founder1', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 'T0', 'Write spec.md', 'Describe deployment steps.', 'in_progress', 'eng1', unixepoch(), unixepoch())",
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
async fn engineer_fixture_full_sequence_lands_artifact() {
    let (pool, layout, graph, _dir) = fixture().await;

    let mut w = WorldView::default();
    w.avatars.insert("founder1".to_string(), av("founder1", "founder"));
    w.avatars.insert("eng1".to_string(), av("eng1", "engineer"));

    let mut paths: PathStore = HashMap::new();
    let mut out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (founder_tx, mut founder_rx) = mpsc::channel(8);
    out_bus.insert("founder1".to_string(), founder_tx);
    let (event_tx, mut event_rx) = make_event_tx();

    // Workspace dir for the writeFile step. Must exist for sandbox::resolve to
    // canonicalize the root.
    let ws_root = std::path::PathBuf::from("workspaces/s1");
    let artifacts = ws_root.join("artifacts");
    tokio::fs::create_dir_all(&artifacts).await.unwrap();

    // Step 1: hypothesis_state.
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
            "type": "mcp_call", "v": 1, "tool": "hypothesis_state", "corr_id": "c1",
            "args": {
                "task_id": "T1",
                "id": "H1",
                "claim": "spec.md exists and contains a 'Goal' section",
                "rationale": "engineer task spec demands a written spec doc",
            }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "hypothesis_state should succeed: {r}");

    // Step 2: writeFile (simulated — Phase 0 worker fixture would do this via
    // its own tool_use loop; here we materialize the canonical artifact).
    let artifact_path = artifacts.join("T1.md");
    tokio::fs::write(&artifact_path, "# Spec\n\n## Goal\n\nWrite the cliptown spec.\n")
        .await
        .unwrap();

    // Step 3: verify (read_assert). The world handler reads `params.contains`.
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
            "type": "mcp_call", "v": 1, "tool": "verify", "corr_id": "c2",
            "args": {
                "method": "read_assert",
                "params": { "path": "artifacts/T1.md", "contains": "Goal" },
            }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "verify should succeed: {r}");
    assert_eq!(r["result"]["observed"]["ok"], true);

    // Step 4: test_record (passed).
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
            "type": "mcp_call", "v": 1, "tool": "test_record", "corr_id": "c3",
            "args": {
                "task_id": "T1",
                "hypothesis_id": "H1",
                "id": "R1",
                "method": "read_assert",
                "params": { "path": "artifacts/T1.md", "contains": "Goal" },
                "expected": "contains 'Goal'",
                "observed": "contains 'Goal'",
                "outcome": "passed",
            }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "test_record should succeed: {r}");

    // Step 5: hypothesis_resolve.
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
            "type": "mcp_call", "v": 1, "tool": "hypothesis_resolve", "corr_id": "c4",
            "args": {
                "task_id": "T1",
                "id": "H1",
                "status": "supported",
                "note": "verify passed",
            }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "hypothesis_resolve should succeed: {r}");

    // Step 6: task_done — uses the canonical artifact path.
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
            "type": "mcp_call", "v": 1, "tool": "task_done", "corr_id": "c5",
            "args": {
                "task_id": "T1",
                "artifact_path": "workspaces/s1/artifacts/T1.md",
            }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "task_done should succeed: {r}");
    assert_eq!(r["result"]["new_status"], "awaiting_review");

    // ── Assertions ──────────────────────────────────────────────────────────
    let row: (String, String) =
        sqlx::query_as("SELECT status, audit_trail FROM tasks WHERE id = 'T1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "awaiting_review");
    assert!(
        row.1.contains("task_done"),
        "audit_trail should contain task_done entry: {}",
        row.1
    );

    // Artifact file exists at canonical path.
    assert!(artifact_path.exists(), "canonical artifact must exist on disk");

    // Epistemic log has all three kinds.
    let epistemic: (Option<String>,) =
        sqlx::query_as("SELECT epistemic_log FROM tasks WHERE id = 'T1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let log = epistemic.0.unwrap_or_default();
    assert!(log.contains("hypothesis_state"), "log missing hypothesis_state: {log}");
    assert!(log.contains("test_record"), "log missing test_record: {log}");
    assert!(log.contains("hypothesis_resolve"), "log missing hypothesis_resolve: {log}");
    // The fixture defines five epistemic-bearing steps but only three of them
    // (state, record, resolve) write to epistemic_log. Verify cardinality so
    // future regressions that double-write or drop entries are caught.
    let parsed: Value = serde_json::from_str(&log).expect("epistemic_log is valid JSON");
    let arr = parsed.as_array().expect("epistemic_log is an array");
    assert_eq!(arr.len(), 3, "expected 3 epistemic entries, got {}: {log}", arr.len());

    // Founder (parent's assignee) should have received subtask_done.
    let evt = founder_rx.try_recv().expect("founder should have received subtask_done");
    assert_eq!(evt["type"], "subtask_done");
    assert_eq!(evt["child_id"], "T1");
    assert_eq!(evt["artifact_path"], "workspaces/s1/artifacts/T1.md");

    // Cleanup so the workspaces/ dir doesn't leak between test runs.
    let _ = tokio::fs::remove_file(&artifact_path).await;
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn task_done_rejects_non_canonical_path() {
    let (pool, layout, graph, _dir) = fixture().await;
    let mut w = WorldView::default();
    w.avatars.insert("eng1".to_string(), av("eng1", "engineer"));
    let mut paths: PathStore = HashMap::new();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (event_tx, mut event_rx) = make_event_tx();

    // Make sure the workspace root exists so sandbox::resolve isn't the layer
    // that fails — we want bad_artifact_path to bite first.
    let ws_root = std::path::PathBuf::from("workspaces/s1/artifacts");
    tokio::fs::create_dir_all(&ws_root).await.unwrap();

    // Path doesn't match the canonical workspaces/<sid>/artifacts/<tid>.md.
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
            "type": "mcp_call", "v": 1, "tool": "task_done", "corr_id": "c1",
            "args": {
                "task_id": "T1",
                "artifact_path": "workspaces/s1/artifacts/wrong.md",
            }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_error", "non-canonical path should be rejected: {r}");
    assert_eq!(r["code"], "bad_artifact_path", "{r}");

    // Task remains in_progress — no transition fired.
    let row: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id = 'T1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "in_progress");
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}
