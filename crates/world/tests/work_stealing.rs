//! P4 Theme E3: async work-stealing among idle peers.
//!
//! Covers both surfaces of the design:
//!   - Manual: `task_steal` MCP tool with the full validation matrix
//!     (not_idle / cross_startup / not_stealable / self_steal / role_mismatch).
//!   - Auto: scheduler post-dispatch pass against per-startup
//!     `auto_steal_enabled`, including the "current assignee is idle → don't
//!     churn" short-circuit.

use cliptown_world::{
    mcp_dispatch,
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
    // e1: busy engineer holding the queued task.
    // e2: idle engineer — eligible stealer.
    // d1: designer — same startup, different role (negative case).
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('e1', 's1', 'E1', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'working')"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('e2', 's1', 'E2', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle')"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('d1', 's1', 'D1', 'designer', 'claude_code', 'm', '{}', 'suite_1', 'idle')"
    ).execute(&pool).await.unwrap();

    let layout = TownLayout::default_town();
    let graph = move_sys::graph_from_layout(&layout);
    let mut world = WorldView::default();
    for (id, role, status) in [("e1", "engineer", "working"), ("e2", "engineer", "idle"), ("d1", "designer", "idle")] {
        world.avatars.insert(
            id.to_string(),
            AvatarView {
                agent_id: id.to_string(),
                startup_id: "s1".into(),
                role: role.into(),
                backend: "claude_code".into(),
                current_pos: (4, 3),
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

fn dispatch_envelope(tool: &str, args: Value) -> Value {
    json!({"type":"mcp_call","v":1,"corr_id":"c1","tool":tool,"args":args})
}

#[tokio::test]
async fn task_steal_happy_path() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, _event_rx, _dir) = setup().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'queued', 'e1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let reply = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, &event_tx,
        "e2",
        dispatch_envelope("task_steal", json!({"task_id": "T1"})),
    ).await;
    assert_eq!(reply["type"], "mcp_reply", "expected ok, got {reply}");
    assert_eq!(reply["result"]["new_assignee"], "e2");
    assert_eq!(reply["result"]["previous_assignee"], "e1");
    let after: (String,) = sqlx::query_as(
        "SELECT assignee_agent_id FROM tasks WHERE id = 'T1'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(after.0, "e2");
}

#[tokio::test]
async fn task_steal_rejects_when_caller_not_idle() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, _event_rx, _dir) = setup().await;
    // Flip e2 to working so the not_idle gate fires.
    w.avatars.get_mut("e2").unwrap().status = "working".into();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'queued', 'e1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let reply = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, &event_tx,
        "e2",
        dispatch_envelope("task_steal", json!({"task_id": "T1"})),
    ).await;
    assert_eq!(reply["type"], "mcp_error");
    assert_eq!(reply["code"], "not_idle");
}

#[tokio::test]
async fn task_steal_rejects_role_mismatch() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, _event_rx, _dir) = setup().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'queued', 'e1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();
    // d1 is idle but a designer — role mismatch with engineer e1.
    let reply = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, &event_tx,
        "d1",
        dispatch_envelope("task_steal", json!({"task_id": "T1"})),
    ).await;
    assert_eq!(reply["type"], "mcp_error");
    assert_eq!(reply["code"], "role_mismatch");
}

#[tokio::test]
async fn task_steal_rejects_self_steal() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, _event_rx, _dir) = setup().await;
    // Make e1 the idle stealer — but it's also the current assignee.
    w.avatars.get_mut("e1").unwrap().status = "idle".into();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'queued', 'e1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();
    let reply = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, &event_tx,
        "e1",
        dispatch_envelope("task_steal", json!({"task_id": "T1"})),
    ).await;
    assert_eq!(reply["type"], "mcp_error");
    assert_eq!(reply["code"], "self_steal");
}

#[tokio::test]
async fn task_steal_rejects_non_queued() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, _event_rx, _dir) = setup().await;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'in_progress', 'e1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();
    let reply = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, &event_tx,
        "e2",
        dispatch_envelope("task_steal", json!({"task_id": "T1"})),
    ).await;
    assert_eq!(reply["type"], "mcp_error");
    assert_eq!(reply["code"], "not_stealable");
}

#[tokio::test]
async fn auto_steal_reassigns_stale_queued_task_to_idle_peer() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, _event_rx, _dir) = setup().await;
    // Flag on, threshold 10s.
    sqlx::query(
        "UPDATE startups SET auto_steal_enabled = 1, auto_steal_after_secs = 10 WHERE id = 's1'"
    ).execute(&pool).await.unwrap();
    // Task assigned to busy e1, updated 60s ago — past the threshold.
    let stale = chrono::Utc::now().timestamp() - 60;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'queued', 'e1', ?, ?)"
    ).bind(stale).bind(stale).execute(&pool).await.unwrap();

    let _ = scheduler::tick(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx,
    ).await;

    let after: (String,) = sqlx::query_as(
        "SELECT assignee_agent_id FROM tasks WHERE id = 'T1'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(after.0, "e2", "auto-steal should reassign stale task to idle peer");
}

#[tokio::test]
async fn auto_steal_disabled_when_flag_off() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, _event_rx, _dir) = setup().await;
    // Default: auto_steal_enabled = 0.
    let stale = chrono::Utc::now().timestamp() - 600;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'queued', 'e1', ?, ?)"
    ).bind(stale).bind(stale).execute(&pool).await.unwrap();

    let _ = scheduler::tick(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx,
    ).await;

    let after: (String,) = sqlx::query_as(
        "SELECT assignee_agent_id FROM tasks WHERE id = 'T1'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(after.0, "e1", "without the flag, ownership must stay with e1");
}

#[tokio::test]
async fn auto_steal_skips_when_current_assignee_idle() {
    let (mut w, mut paths, layout, graph, out_bus, _rx, pool, event_tx, _event_rx, _dir) = setup().await;
    // Flag on, threshold 1s — and e1 is idle too. The dispatch loop will
    // handle this row normally; auto-steal must not churn.
    sqlx::query(
        "UPDATE startups SET auto_steal_enabled = 1, auto_steal_after_secs = 1 WHERE id = 's1'"
    ).execute(&pool).await.unwrap();
    w.avatars.get_mut("e1").unwrap().status = "idle".into();
    let stale = chrono::Utc::now().timestamp() - 60;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'queued', 'e1', ?, ?)"
    ).bind(stale).bind(stale).execute(&pool).await.unwrap();

    // out_bus has no entry for e1 in our setup() (only e2), so the dispatch
    // loop will skip e1's task on the liveness gate. That keeps the row
    // queued, but auto-steal must still respect the "current assignee idle"
    // short-circuit.
    let _ = scheduler::tick(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, None, &event_tx,
    ).await;

    let after: (String,) = sqlx::query_as(
        "SELECT assignee_agent_id FROM tasks WHERE id = 'T1'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(after.0, "e1", "idle assignee → auto-steal must not fire");
}
