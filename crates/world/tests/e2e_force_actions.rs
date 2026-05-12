//! M6.3 end-to-end integration test: operator force_accept and
//! force_fail kanban actions.
//!
//! Phase 0 plan §6.3 — these are the operator-only "drag" actions
//! emitted by the future /console kanban UI. M1.12 already wired
//! `OperatorForceAccept` and `OperatorForceFail` through
//! `cmd_console::dispatch`; this suite verifies the contract:
//!
//! - `force_accept`: AwaitingReview → Done (illegal from any other
//!   non-terminal state per task_sm).
//! - `force_fail`: any non-terminal status → Failed (rejected from
//!   Done/Failed). Requires a `note`, recorded in the audit trail.
//! - Both actions append `{"actor":"operator", ...}` audit events.

mod common;

use cliptown_world::{
    cmd_console, protocol::ConsoleOutbound, seed,
    state::{AvatarView, WorldView},
    storage,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::{broadcast, mpsc};

async fn fixture() -> sqlx::SqlitePool {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("test.db");
    let pool = storage::open(p.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    // Keep tempdir alive for the test process lifetime (parity with
    // sibling e2e suites that need the sqlite file on disk).
    std::mem::forget(dir);

    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'alpha', 'goal', 10.0, 'town_default', 'workspaces/s1', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status, manager_id) \
         VALUES ('eng1', 's1', 'E', 'engineer', 'claude_code', '', '{}', 'suite_1', 'idle', NULL)"
    ).execute(&pool).await.unwrap();
    pool
}

fn fresh_world() -> WorldView {
    let mut w = WorldView::default();
    w.avatars.insert(
        "eng1".to_string(),
        AvatarView {
            agent_id: "eng1".into(),
            startup_id: "s1".into(),
            role: "engineer".into(),
            backend: "claude_code".into(),
            current_pos: (3, 3),
            target_pos: None,
            room_id: "suite_1".into(),
            status: "idle".into(),
            last_seen_at: None,
            health: cliptown_world::health::Health::Offline,
        },
    );
    w
}

fn make_event_tx() -> (broadcast::Sender<ConsoleOutbound>, broadcast::Receiver<ConsoleOutbound>) {
    broadcast::channel(64)
}

#[tokio::test]
async fn force_accept_awaiting_review_to_done() {
    let pool = fixture().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 'review me', 'desc', 'awaiting_review', 'eng1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let mut w = fresh_world();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (event_tx, mut event_rx) = make_event_tx();

    let r = cmd_console::dispatch(
        &mut w,
        &pool,
        &out_bus,
        &event_tx,
        json!({ "type": "operator_force_accept", "v": 1, "task_id": "T1" }),
    )
    .await;
    assert_eq!(r["type"], "ok", "force_accept: {:?}", r);

    let row: (String, String) = sqlx::query_as(
        "SELECT status, audit_trail FROM tasks WHERE id = 'T1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "done");
    assert!(row.1.contains("force_accept"));
    assert!(row.1.contains("\"actor\":\"operator\""));
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn force_accept_from_in_progress_rejected() {
    let pool = fixture().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 'in flight', 'desc', 'in_progress', 'eng1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let mut w = fresh_world();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (event_tx, mut event_rx) = make_event_tx();

    // ForceAccept is only legal from awaiting_review per task_sm.
    let r = cmd_console::dispatch(
        &mut w,
        &pool,
        &out_bus,
        &event_tx,
        json!({ "type": "operator_force_accept", "v": 1, "task_id": "T1" }),
    )
    .await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "illegal_transition");

    let row: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id = 'T1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "in_progress");
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn force_fail_from_queued_to_failed_with_note() {
    let pool = fixture().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 'queued task', 'desc', 'queued', 'eng1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let mut w = fresh_world();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (event_tx, mut event_rx) = make_event_tx();

    let r = cmd_console::dispatch(
        &mut w,
        &pool,
        &out_bus,
        &event_tx,
        json!({
            "type": "operator_force_fail", "v": 1,
            "task_id": "T1",
            "note": "abandoned by operator"
        }),
    )
    .await;
    assert_eq!(r["type"], "ok");

    let row: (String, String) = sqlx::query_as(
        "SELECT status, audit_trail FROM tasks WHERE id = 'T1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "failed");
    assert!(row.1.contains("force_fail"));
    assert!(row.1.contains("abandoned by operator"));
    assert!(row.1.contains("\"actor\":\"operator\""));
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn force_fail_from_in_progress() {
    let pool = fixture().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 'doomed', 'desc', 'in_progress', 'eng1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let mut w = fresh_world();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (event_tx, mut event_rx) = make_event_tx();

    let r = cmd_console::dispatch(
        &mut w,
        &pool,
        &out_bus,
        &event_tx,
        json!({ "type": "operator_force_fail", "v": 1, "task_id": "T1", "note": "x" }),
    )
    .await;
    assert_eq!(r["type"], "ok");
    let row: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id = 'T1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "failed");
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn force_fail_already_done_rejected() {
    let pool = fixture().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 'closed', 'desc', 'done', 'eng1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let mut w = fresh_world();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (event_tx, mut event_rx) = make_event_tx();

    let r = cmd_console::dispatch(
        &mut w,
        &pool,
        &out_bus,
        &event_tx,
        json!({ "type": "operator_force_fail", "v": 1, "task_id": "T1", "note": "too late" }),
    )
    .await;
    assert_eq!(r["type"], "error");

    let row: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id = 'T1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "done");
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}
