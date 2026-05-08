//! M5.7 — Permission violation rejected with alert event.
//!
//! Drives `mcp_dispatch::dispatch` for a `move_intent` against a room that is
//! owned by a different startup. Asserts:
//!   1. The wire reply is `mcp_error { code: "no_permission" }` (unchanged
//!      from M1.13's StartMoveResult::PermissionDenied contract).
//!   2. A `system_events` row lands with `kind = 'permission_violation'`,
//!      `severity = 'alert'`, and a payload that records who tried what.
//!   3. A successful move into a public room (lobby) does NOT emit any
//!      permission_violation row, so the alert is precisely targeted.
//!
//! The fixture mirrors the in-memory `TownLayout` ownership against the SQL
//! `rooms.private_to_startup_id`. The runtime permission check sits on the
//! layout (`move_sys::can_enter_layout_room`), so the layout mutation is the
//! load-bearing one — but we keep the SQL aligned so any future code path
//! that derives ownership from the DB still observes the same world.

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
    let p = dir.path().join("e2e-permission-violation.db");
    let pool = storage::open(p.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();

    // α (s_alpha) and β (s_beta) startups; suite_1 owned by α, suite_2 owned
    // by β. The α-engineer trying to enter suite_2 is the violation under test.
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s_alpha', 'Alpha', 'g', 5.0, 'town_default', 'workspaces/s_alpha', 'active', unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s_beta', 'Beta', 'g', 5.0, 'town_default', 'workspaces/s_beta', 'active', unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE rooms SET private_to_startup_id = 's_alpha' WHERE id = 'suite_1'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE rooms SET private_to_startup_id = 's_beta' WHERE id = 'suite_2'")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('alpha_eng', 's_alpha', 'AE', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let mut layout = TownLayout::default_town();
    for r in layout.rooms.iter_mut() {
        match r.id.as_str() {
            "suite_1" => r.private_to_startup_id = Some("s_alpha".into()),
            "suite_2" => r.private_to_startup_id = Some("s_beta".into()),
            _ => {}
        }
    }
    let graph = move_sys::graph_from_layout(&layout);
    (pool, layout, graph, dir)
}

fn alpha_engineer() -> AvatarView {
    AvatarView {
        agent_id: "alpha_eng".into(),
        startup_id: "s_alpha".into(),
        role: "engineer".into(),
        backend: "claude_code".into(),
        // Inside suite_1's bounds (0,0,7,6).
        current_pos: (3, 3),
        target_pos: None,
        room_id: "suite_1".into(),
        status: "idle".into(),
    }
}

fn make_event_tx() -> (broadcast::Sender<ConsoleOutbound>, broadcast::Receiver<ConsoleOutbound>) {
    broadcast::channel(64)
}

#[tokio::test]
async fn alpha_engineer_cannot_enter_beta_suite() {
    let (pool, layout, graph, _dir) = fixture().await;
    let mut w = WorldView::default();
    w.avatars.insert("alpha_eng".to_string(), alpha_engineer());
    let mut paths: PathStore = HashMap::new();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (event_tx, mut event_rx) = make_event_tx();

    // α-engineer attempts to move into suite_2 (β-owned).
    let r = mcp_dispatch::dispatch(
        &mut w,
        &mut paths,
        &layout,
        &graph,
        &out_bus,
        &pool,
        &event_tx,
        "alpha_eng",
        json!({
            "type": "mcp_call", "v": 1, "tool": "move_intent", "corr_id": "c1",
            "args": { "target_room": "suite_2" }
        }),
    )
    .await;

    assert_eq!(r["type"], "mcp_error", "permission denied should produce mcp_error: {r}");
    assert_eq!(r["code"], "no_permission", "expected no_permission code: {r}");

    // Exactly one permission_violation system_events row, severity = 'alert'.
    let count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM system_events \
         WHERE kind = 'permission_violation' AND severity = 'alert'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count.0, 1, "expected 1 alert row, got {}", count.0);

    // Payload should attribute the violation to the calling agent + target room.
    let row: (String, Option<String>) = sqlx::query_as(
        "SELECT payload, startup_id FROM system_events \
         WHERE kind = 'permission_violation' LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let payload: Value = serde_json::from_str(&row.0).expect("payload must be valid JSON");
    assert_eq!(payload["agent_id"], "alpha_eng", "payload missing agent_id: {}", row.0);
    assert_eq!(payload["target_room"], "suite_2", "payload missing target_room: {}", row.0);
    assert_eq!(row.1.as_deref(), Some("s_alpha"), "alert should be tagged to caller's startup");

    // Caller-path assertion (M16): emit_system_event broadcasts a frame to the
    // operator console in addition to writing the SQL row.
    let frame = event_rx.try_recv().expect("expected SystemEvent broadcast for permission_violation");
    let ConsoleOutbound::SystemEvent { kind, severity, .. } = frame else {
        panic!("expected SystemEvent, got something else");
    };
    assert_eq!(kind, "permission_violation");
    assert_eq!(severity, "alert");
}

#[tokio::test]
async fn alpha_engineer_can_enter_public_room() {
    let (pool, layout, graph, _dir) = fixture().await;
    let mut w = WorldView::default();
    w.avatars.insert("alpha_eng".to_string(), alpha_engineer());
    let mut paths: PathStore = HashMap::new();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (event_tx, mut event_rx) = make_event_tx();

    // Lobby is public — no ownership, should succeed.
    let r = mcp_dispatch::dispatch(
        &mut w,
        &mut paths,
        &layout,
        &graph,
        &out_bus,
        &pool,
        &event_tx,
        "alpha_eng",
        json!({
            "type": "mcp_call", "v": 1, "tool": "move_intent", "corr_id": "c1",
            "args": { "target_room": "lobby" }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "public room move should succeed: {r}");

    // No alert row should have been written for a successful move.
    let count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM system_events WHERE kind = 'permission_violation'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 0, "no permission_violation rows expected on success");
    assert!(matches!(event_rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
}
