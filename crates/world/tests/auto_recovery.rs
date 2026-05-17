//! P6 Theme C: auto-recovery pass integration tests.

use cliptown_world::{
    move_sys::{self, PathStore},
    path::RoomGraph,
    scheduler,
    seed::{self, TownLayout},
    state::{AvatarView, WorldView},
    storage,
};
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::mpsc;

async fn setup() -> (
    WorldView,
    PathStore,
    TownLayout,
    RoomGraph,
    HashMap<String, mpsc::Sender<Value>>,
    mpsc::Receiver<Value>,
    sqlx::SqlitePool,
    tokio::sync::broadcast::Sender<cliptown_world::protocol::ConsoleOutbound>,
    tokio::sync::broadcast::Receiver<cliptown_world::protocol::ConsoleOutbound>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let pool = storage::open(dir.path().join("t.db").to_str().unwrap())
        .await
        .unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'a', 'g', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    // Two engineers in the same startup so the recovery pass can pick
    // a peer.
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('e1', 's1', 'E1', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'working')"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('e2', 's1', 'E2', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle')"
    ).execute(&pool).await.unwrap();

    let layout = TownLayout::default_town();
    let graph = move_sys::graph_from_layout(&layout);
    let mut world = WorldView::default();
    for (id, status) in [("e1", "working"), ("e2", "idle")] {
        world.avatars.insert(
            id.into(),
            AvatarView {
                agent_id: id.into(),
                startup_id: "s1".into(),
                role: "engineer".into(),
                backend: "claude_code".into(),
                current_pos: (0, 0),
                target_pos: None,
                room_id: "suite_1".into(),
                status: status.into(),
                last_seen_at: None,
                health: cliptown_world::health::Health::Online,
            },
        );
    }
    let mut out_bus = HashMap::new();
    let (tx, rx) = mpsc::channel::<Value>(8);
    out_bus.insert("e2".to_string(), tx);
    let (event_tx, event_rx) = tokio::sync::broadcast::channel(64);
    (world, PathStore::new(), layout, graph, out_bus, rx, pool, event_tx, event_rx, dir)
}

fn drain_kinds(
    rx: &mut tokio::sync::broadcast::Receiver<cliptown_world::protocol::ConsoleOutbound>,
) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(cliptown_world::protocol::ConsoleOutbound::SystemEvent { kind, payload, .. }) => {
                out.push((kind, payload));
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    out
}

#[tokio::test]
async fn auto_recovery_reassigns_after_max_attempts() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, mut event_rx, _dir) =
        setup().await;
    sqlx::query("UPDATE startups SET auto_recovery_enabled = 1, auto_recovery_max_attempts = 2 WHERE id = 's1'")
        .execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, review_round, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'changes_requested', 'e1', 2, unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let _ = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx).await;

    let row: (String, String, i64) = sqlx::query_as(
        "SELECT assignee_agent_id, status, review_round FROM tasks WHERE id='T1'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(row.0, "e2", "task must be reassigned to the idle peer");
    assert_eq!(row.1, "queued", "status must reset so scheduler dispatches");
    assert_eq!(row.2, 0, "review_round must reset");
    let events = drain_kinds(&mut event_rx);
    assert!(events.iter().any(|(k, _)| k == "task_recovered"),
        "task_recovered system_event must fire");
}

#[tokio::test]
async fn auto_recovery_skips_under_threshold() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, mut event_rx, _dir) =
        setup().await;
    sqlx::query("UPDATE startups SET auto_recovery_enabled = 1, auto_recovery_max_attempts = 3 WHERE id = 's1'")
        .execute(&pool).await.unwrap();
    // review_round = 2 but max = 3 → must not fire yet.
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, review_round, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'changes_requested', 'e1', 2, unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let _ = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx).await;

    let row: (String, String) = sqlx::query_as(
        "SELECT assignee_agent_id, status FROM tasks WHERE id='T1'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(row.0, "e1", "under threshold → no reassign");
    assert_eq!(row.1, "changes_requested");
    let events = drain_kinds(&mut event_rx);
    assert!(events.iter().all(|(k, _)| k != "task_recovered"));
}

#[tokio::test]
async fn auto_recovery_disabled_is_a_noop() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, mut event_rx, _dir) =
        setup().await;
    // Flag stays off (default 0). Task is past the default threshold.
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, review_round, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'changes_requested', 'e1', 5, unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let _ = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx).await;

    let row: (String,) = sqlx::query_as(
        "SELECT assignee_agent_id FROM tasks WHERE id='T1'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(row.0, "e1");
    let events = drain_kinds(&mut event_rx);
    assert!(events.iter().all(|(k, _)| k != "task_recovered"));
}

#[tokio::test]
async fn auto_recovery_skips_when_no_idle_peer() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, mut event_rx, _dir) =
        setup().await;
    sqlx::query("UPDATE startups SET auto_recovery_enabled = 1, auto_recovery_max_attempts = 1 WHERE id = 's1'")
        .execute(&pool).await.unwrap();
    // Flip e2 to working so no idle peer remains.
    w.avatars.get_mut("e2").unwrap().status = "working".into();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, review_round, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'changes_requested', 'e1', 1, unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let _ = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx).await;

    let row: (String, String) = sqlx::query_as(
        "SELECT assignee_agent_id, status FROM tasks WHERE id='T1'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(row.0, "e1", "no idle peer → assignment stays");
    assert_eq!(row.1, "changes_requested");
    let events = drain_kinds(&mut event_rx);
    assert!(events.iter().all(|(k, _)| k != "task_recovered"));
}
