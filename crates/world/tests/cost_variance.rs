//! P3 Theme C follow-up: cost variance telemetry tests.
//!
//! Drives `cmd_worker::dispatch` with a `report_budget` payload and asserts
//! that the world emits `task_cost_variance` system_events when actual vs
//! estimate diverges by ±50%, and skips emission below the threshold.

use cliptown_world::{
    cmd_worker, move_sys::{self, PathStore}, path::RoomGraph, seed::{self, TownLayout},
    state::{AvatarView, WorldView}, storage,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::mpsc;

async fn setup() -> (
    WorldView,
    PathStore,
    TownLayout,
    RoomGraph,
    HashMap<String, mpsc::Sender<Value>>,
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
         VALUES ('s1', 'alpha', 'g', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('e1', 's1', 'E1', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'working')"
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
            status: "working".into(),
            last_seen_at: None,
            health: cliptown_world::health::Health::Online,
        },
    );
    let (event_tx, event_rx) = tokio::sync::broadcast::channel(64);
    (
        world,
        PathStore::new(),
        layout,
        graph,
        HashMap::new(),
        pool,
        event_tx,
        event_rx,
        dir,
    )
}

async fn report_budget(
    pool: &sqlx::SqlitePool,
    world: &mut WorldView,
    paths: &mut PathStore,
    layout: &TownLayout,
    graph: &RoomGraph,
    out_bus: &HashMap<String, mpsc::Sender<Value>>,
    event_tx: &tokio::sync::broadcast::Sender<cliptown_world::protocol::ConsoleOutbound>,
    task_id: &str,
    cost_usd: f64,
) -> Value {
    cmd_worker::dispatch(
        world, paths, layout, graph, out_bus, pool, event_tx, "e1",
        json!({
            "type": "report_budget",
            "v": 1,
            "in_tokens": 100,
            "out_tokens": 50,
            "model_id": "claude-haiku-4-5",
            "task_id": task_id,
            "cost_usd": cost_usd,
        }),
    )
    .await
}

fn drain_system_events(
    rx: &mut tokio::sync::broadcast::Receiver<cliptown_world::protocol::ConsoleOutbound>,
) -> Vec<(String, String, Value)> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(cliptown_world::protocol::ConsoleOutbound::SystemEvent {
                kind, severity, payload, ..
            }) => out.push((kind, severity, payload)),
            Ok(_) => continue,
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(_) => break,
        }
    }
    out
}

#[tokio::test]
async fn overrun_above_50pct_emits_warn_variance_event() {
    let (mut w, mut paths, layout, graph, bus, pool, event_tx, mut event_rx, _dir) = setup().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, cost_estimate_usd, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'in_progress', 'e1', 0.10, unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let r = report_budget(&pool, &mut w, &mut paths, &layout, &graph, &bus, &event_tx, "T1", 0.18).await;
    assert_eq!(r["type"], "ok", "{r}");

    let events: Vec<_> = drain_system_events(&mut event_rx).into_iter()
        .filter(|(k, _, _)| k == "task_cost_variance")
        .collect();
    assert_eq!(events.len(), 1, "expected one variance event, got {events:?}");
    let (_, severity, payload) = &events[0];
    assert_eq!(severity, "warn", "overrun should be warn");
    // payload is already a parsed Value (emit_system_event re-parses the str).
    assert_eq!(payload["task_id"], "T1");
    assert!(payload["delta_pct"].as_f64().unwrap() >= 50.0);
}

#[tokio::test]
async fn underrun_below_50pct_emits_info_variance_event() {
    let (mut w, mut paths, layout, graph, bus, pool, event_tx, mut event_rx, _dir) = setup().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, cost_estimate_usd, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'in_progress', 'e1', 0.20, unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let r = report_budget(&pool, &mut w, &mut paths, &layout, &graph, &bus, &event_tx, "T1", 0.05).await;
    assert_eq!(r["type"], "ok");

    let events: Vec<_> = drain_system_events(&mut event_rx).into_iter()
        .filter(|(k, _, _)| k == "task_cost_variance")
        .collect();
    assert_eq!(events.len(), 1);
    let (_, severity, payload) = &events[0];
    assert_eq!(severity, "info", "underrun is informational");
    // payload is already a parsed Value (emit_system_event re-parses the str).
    assert!(payload["delta_pct"].as_f64().unwrap() <= -50.0);
}

#[tokio::test]
async fn within_50pct_skips_variance_event() {
    let (mut w, mut paths, layout, graph, bus, pool, event_tx, mut event_rx, _dir) = setup().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, cost_estimate_usd, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'in_progress', 'e1', 0.10, unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();
    let r = report_budget(&pool, &mut w, &mut paths, &layout, &graph, &bus, &event_tx, "T1", 0.12).await;
    assert_eq!(r["type"], "ok");
    let events: Vec<_> = drain_system_events(&mut event_rx).into_iter()
        .filter(|(k, _, _)| k == "task_cost_variance")
        .collect();
    assert!(events.is_empty(), "20% delta should not fire, got {events:?}");
}

#[tokio::test]
async fn missing_estimate_skips_variance_event() {
    let (mut w, mut paths, layout, graph, bus, pool, event_tx, mut event_rx, _dir) = setup().await;
    // No cost_estimate_usd set on the task.
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'in_progress', 'e1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();
    let r = report_budget(&pool, &mut w, &mut paths, &layout, &graph, &bus, &event_tx, "T1", 5.0).await;
    assert_eq!(r["type"], "ok");
    let events: Vec<_> = drain_system_events(&mut event_rx).into_iter()
        .filter(|(k, _, _)| k == "task_cost_variance")
        .collect();
    assert!(events.is_empty(), "no estimate → no variance, got {events:?}");
}
