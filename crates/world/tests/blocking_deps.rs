//! P4 Theme E2: blocking dependencies + deadlines.

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
    // Receiver held by the caller to keep the channel open — dropping it
    // closes the sender and `try_send` fails with `Closed`, which the
    // scheduler treats as a dispatch failure.
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
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('e1', 's1', 'E1', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle')"
    ).execute(&pool).await.unwrap();

    let layout = TownLayout::default_town();
    let graph = move_sys::graph_from_layout(&layout);
    let mut world = WorldView::default();
    world.avatars.insert(
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
            health: cliptown_world::health::Health::Online,
        },
    );
    let mut out_bus = HashMap::new();
    let (tx, rx) = mpsc::channel::<Value>(8);
    out_bus.insert("e1".to_string(), tx);
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
async fn blocked_task_not_dispatched_while_blocker_in_progress() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, _event_rx, _dir) = setup().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T_block', 's1', 'blocker', 'd', 'in_progress', 'e1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, blocked_on, created_at, updated_at) \
         VALUES ('T_held', 's1', 'held', 'd', 'queued', 'e1', 'T_block', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let n = scheduler::tick(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx,
    ).await;
    assert_eq!(n, 0, "blocked task must not dispatch while blocker is non-terminal");
    let s: (String, Option<String>) = sqlx::query_as(
        "SELECT status, blocked_on FROM tasks WHERE id='T_held'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(s.0, "queued");
    assert_eq!(s.1.as_deref(), Some("T_block"));
}

#[tokio::test]
async fn unblock_clears_column_and_emits_event_then_dispatches() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, mut event_rx, _dir) = setup().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T_block', 's1', 'blocker', 'd', 'done', 'e1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, blocked_on, created_at, updated_at) \
         VALUES ('T_held', 's1', 'held', 'd', 'queued', 'e1', 'T_block', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let n = scheduler::tick(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx,
    ).await;
    assert_eq!(n, 1, "blocker done → held should dispatch");
    let after: (String, Option<String>) = sqlx::query_as(
        "SELECT status, blocked_on FROM tasks WHERE id='T_held'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(after.0, "in_progress");
    assert!(after.1.is_none(), "blocked_on must clear once dependency resolves");
    let events = drain_kinds(&mut event_rx);
    assert!(events.iter().any(|(k, _)| k == "task_unblocked"));
}

#[tokio::test]
async fn overdue_task_emits_warn_event_once() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, mut event_rx, _dir) = setup().await;
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, deadline_at, created_at, updated_at) \
         VALUES ('T_late', 's1', 'late', 'd', 'in_progress', 'e1', ?, unixepoch(), unixepoch())"
    ).bind(now - 60).execute(&pool).await.unwrap();

    let _ = scheduler::tick(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx,
    ).await;
    let events1 = drain_kinds(&mut event_rx);
    let overdue1: Vec<_> = events1.iter().filter(|(k, _)| k == "task_overdue").collect();
    assert_eq!(overdue1.len(), 1, "first tick fires the overdue event");
    let (_, payload) = overdue1[0];
    let payload_obj: Value = serde_json::from_str(payload.as_str().unwrap_or("{}")).unwrap_or_else(|_| payload.clone());
    let _ = payload_obj; // payload format covered by emit::emit_system_event tests.

    // Second tick → dedup, no re-emit.
    let _ = scheduler::tick(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx,
    ).await;
    let events2 = drain_kinds(&mut event_rx);
    let overdue2: Vec<_> = events2.iter().filter(|(k, _)| k == "task_overdue").collect();
    assert!(overdue2.is_empty(), "second tick must not re-emit; got {events2:?}");
}

#[tokio::test]
async fn no_deadline_no_overdue_event() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, mut event_rx, _dir) = setup().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'in_progress', 'e1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();
    let _ = scheduler::tick(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx,
    ).await;
    let events = drain_kinds(&mut event_rx);
    assert!(events.iter().all(|(k, _)| k != "task_overdue"));
}

#[tokio::test]
async fn terminal_task_does_not_fire_overdue() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, mut event_rx, _dir) = setup().await;
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, deadline_at, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'done', 'e1', ?, unixepoch(), unixepoch())"
    ).bind(now - 3600).execute(&pool).await.unwrap();
    let _ = scheduler::tick(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx,
    ).await;
    let events = drain_kinds(&mut event_rx);
    assert!(events.iter().all(|(k, _)| k != "task_overdue"),
        "terminal tasks bypass the overdue scan: {events:?}");
}

#[tokio::test]
async fn editing_deadline_via_set_blocking_clears_dedup_and_re_emits() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, mut event_rx, _dir) = setup().await;
    let now = chrono::Utc::now().timestamp();
    // First deadline (1 minute ago) → notified.
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, deadline_at, deadline_notified_at, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'in_progress', 'e1', ?, ?, unixepoch(), unixepoch())"
    ).bind(now - 60).bind(now - 30).execute(&pool).await.unwrap();
    let _ = scheduler::tick(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx,
    ).await;
    let events = drain_kinds(&mut event_rx);
    assert!(events.iter().all(|(k, _)| k != "task_overdue"),
        "row was notified → tick must not re-emit");

    // Simulate the MCP handler clearing dedup on deadline change. (Manually
    // exercise the SQL side; the handler-level test lives in mcp_handlers.)
    sqlx::query(
        "UPDATE tasks SET deadline_at = ?, deadline_notified_at = NULL WHERE id='T1'"
    ).bind(now - 10).execute(&pool).await.unwrap();
    let _ = scheduler::tick(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx,
    ).await;
    let events2 = drain_kinds(&mut event_rx);
    assert!(events2.iter().any(|(k, _)| k == "task_overdue"),
        "after clearing dedup, the new deadline should fire");
}
