//! Unit tests for the task scheduler (M1.14).
//! Drives `scheduler::tick` directly with a fresh in-memory pool + WorldView.

use cliptown_world::{
    move_sys::{self, PathStore},
    path::RoomGraph,
    scheduler,
    seed::{self, TownLayout},
    state::{AvatarView, WorldView},
    storage,
};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Build the standard scheduler-test fixture: pool seeded with default town,
/// one startup `s1` with one engineer agent `a1` (idle, in `suite_1` at
/// (3, 3)), no tasks, empty out_bus.
async fn fixture() -> (
    WorldView,
    PathStore,
    TownLayout,
    RoomGraph,
    HashMap<String, mpsc::Sender<serde_json::Value>>,
    sqlx::SqlitePool,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("scheduler-test.db");
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
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('a1', 's1', 'A1', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let layout = TownLayout::default_town();
    let graph = move_sys::graph_from_layout(&layout);
    let mut w = WorldView::default();
    w.avatars.insert(
        "a1".to_string(),
        AvatarView {
            agent_id: "a1".to_string(),
            startup_id: "s1".to_string(),
            role: "engineer".to_string(),
            backend: "claude_code".to_string(),
            current_pos: (3, 3),
            target_pos: None,
            room_id: "suite_1".to_string(),
            status: "idle".to_string(),
        },
    );
    let paths: PathStore = HashMap::new();
    let out_bus: HashMap<String, mpsc::Sender<serde_json::Value>> = HashMap::new();
    (w, paths, layout, graph, out_bus, pool, dir)
}

#[tokio::test]
async fn queued_idle_transitions_to_in_progress() {
    let (mut w, mut paths, layout, graph, out_bus, pool, _dir) = fixture().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 'task', 'desc', 'queued', 'a1', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();

    let n = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool).await;
    assert_eq!(n, 1);

    let s: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(s.0, "in_progress");
    // Agent is now flagged busy in-memory.
    assert_eq!(w.avatars["a1"].status, "working");
}

#[tokio::test]
async fn queued_working_agent_does_not_dispatch() {
    let (mut w, mut paths, layout, graph, out_bus, pool, _dir) = fixture().await;
    if let Some(a) = w.avatars.get_mut("a1") {
        a.status = "working".to_string();
    }
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 'task', 'desc', 'queued', 'a1', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();

    let n = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool).await;
    assert_eq!(n, 0);

    let s: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(s.0, "queued");
}

#[tokio::test]
async fn required_room_triggers_move_not_dispatch() {
    // Agent starts in suite_1; required_room is library (different room).
    // Scheduler should kick off a move and leave the task queued.
    let (mut w, mut paths, layout, graph, out_bus, pool, _dir) = fixture().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, required_room, created_at, updated_at) \
         VALUES ('T1', 's1', 'task', 'desc', 'queued', 'a1', 'library', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();

    let n = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool).await;
    // No dispatch yet — move was started instead.
    assert_eq!(n, 0);
    assert!(
        paths.contains_key("a1"),
        "scheduler should have started a move toward library"
    );
    assert!(
        w.avatars["a1"].target_pos.is_some(),
        "target_pos should be set after start_move"
    );

    let s: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(s.0, "queued");
}

#[tokio::test]
async fn required_room_already_satisfied_dispatches() {
    let (mut w, mut paths, layout, graph, out_bus, pool, _dir) = fixture().await;
    if let Some(a) = w.avatars.get_mut("a1") {
        a.room_id = "library".to_string();
        a.current_pos = (15, 10);
    }
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, required_room, created_at, updated_at) \
         VALUES ('T1', 's1', 'task', 'desc', 'queued', 'a1', 'library', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();

    let n = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool).await;
    assert_eq!(n, 1);
    // No move should have been kicked off since agent already in library.
    assert!(!paths.contains_key("a1"));

    let s: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(s.0, "in_progress");
}

#[tokio::test]
async fn dispatched_task_pushes_task_assigned_to_out_bus() {
    let (mut w, mut paths, layout, graph, mut out_bus, pool, _dir) = fixture().await;
    let (tx, mut rx) = mpsc::channel::<serde_json::Value>(8);
    out_bus.insert("a1".to_string(), tx);

    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 'task', 'desc', 'queued', 'a1', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();

    let n = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool).await;
    assert_eq!(n, 1);

    let msg = rx
        .try_recv()
        .expect("task_assigned should have been pushed to out_bus");
    // Round-trip via the protocol type — this is the contract a real worker uses.
    let parsed: cliptown_world::protocol::WorkerOutbound =
        serde_json::from_value(msg).expect("payload must deserialize as WorkerOutbound");
    match parsed {
        cliptown_world::protocol::WorkerOutbound::TaskAssigned {
            v,
            task_id,
            title,
            description,
            required_room,
            parent_id,
        } => {
            assert_eq!(v, 1);
            assert_eq!(task_id, "T1");
            assert_eq!(title, "task");
            assert_eq!(description, "desc");
            assert!(required_room.is_none());
            assert!(parent_id.is_none());
        }
        other => panic!("expected TaskAssigned, got {:?}", other),
    }
}

#[tokio::test]
async fn dispatched_task_writes_audit_trail() {
    let (mut w, mut paths, layout, graph, out_bus, pool, _dir) = fixture().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 'task', 'desc', 'queued', 'a1', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();
    let _ = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool).await;
    let row: (String,) = sqlx::query_as("SELECT audit_trail FROM tasks WHERE id='T1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(row.0.contains("task_assigned"));
    assert!(row.0.contains("scheduler"));
}

#[tokio::test]
async fn unreachable_required_room_logs_warn_and_skips_dispatch() {
    // Agent a1 belongs to s1, but the task requires entering suite_2 — which
    // we mark private to s2. `move_sys::start_move` returns PermissionDenied;
    // the scheduler logs a warn and skips dispatch. Task remains queued and
    // no path is created.
    let (mut w, mut paths, mut layout, graph, out_bus, pool, _dir) = fixture().await;
    // Mark suite_2 as private to s2 in the in-memory layout. (default_town()
    // seeds private_to_startup_id = None for every room.)
    if let Some(r) = layout.rooms.iter_mut().find(|r| r.id == "suite_2") {
        r.private_to_startup_id = Some("s2".to_string());
    }
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s2', 'beta', 'goal', 10.0, 'town_default', '/tmp/s2', 'active', unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, required_room, created_at, updated_at) \
         VALUES ('T1', 's1', 'task', 'desc', 'queued', 'a1', 'suite_2', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();

    let n = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool).await;
    assert_eq!(n, 0);
    assert!(
        !paths.contains_key("a1"),
        "no path should be created when start_move returns PermissionDenied"
    );

    let s: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(s.0, "queued");
}

#[tokio::test]
async fn queued_without_assignee_is_ignored() {
    // Task is queued but has no assignee. Scheduler should not pick it up.
    let (mut w, mut paths, layout, graph, out_bus, pool, _dir) = fixture().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, created_at, updated_at) \
         VALUES ('T1', 's1', 'task', 'desc', 'queued', unixepoch(), unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();

    let n = scheduler::tick(&mut w, &mut paths, &layout, &graph, &out_bus, &pool).await;
    assert_eq!(n, 0);

    let s: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(s.0, "queued");
}
