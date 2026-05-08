//! M7.2 — Invariant 7: cross-startup cafe chat delivered.
//!
//! When agents from different startups stand in a public room (cafe/lobby/
//! library), `speak {kind:"chat"}` fans across the startup boundary so they
//! can overhear each other. Private suites keep the same-startup-only rule.
//!
//! These tests drive the post-arrival state directly (avatars placed in
//! cafe) rather than walking the full move_intent pathing — that's covered
//! by movement.rs / e2e_engineer_artifact.rs. Here we focus on the chat
//! broadcast filter (mcp_dispatch::handle_speak) and its interaction with
//! the public/private room boundary.

use cliptown_world::{
    mcp_dispatch,
    move_sys::{self, PathStore},
    path,
    seed::{self, TownLayout},
    state::{AvatarView, WorldView},
    storage,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::mpsc;

async fn fixture() -> (sqlx::SqlitePool, TownLayout, path::RoomGraph) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("test.db");
    let pool = storage::open(p.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    // Keep tempdir alive for the duration of the test process.
    std::mem::forget(dir);

    for sid in &["alpha", "beta"] {
        sqlx::query(
            "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
             VALUES (?, ?, 'g', 5.0, 'town_default', ?, 'active', unixepoch())"
        ).bind(sid).bind(sid).bind(format!("workspaces/{}", sid))
         .execute(&pool).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('alpha_eng', 'alpha', 'AE', 'engineer', 'claude_code', '', '{}', 'suite_1', 'idle')"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('beta_des', 'beta', 'BD', 'designer', 'claude_code', '', '{}', 'suite_2', 'idle')"
    ).execute(&pool).await.unwrap();

    let layout = TownLayout::default_town();
    let graph = move_sys::graph_from_layout(&layout);
    (pool, layout, graph)
}

fn av_in(id: &str, sid: &str, role: &str, room: &str) -> AvatarView {
    AvatarView {
        agent_id: id.into(),
        startup_id: sid.into(),
        role: role.into(),
        backend: "claude_code".into(),
        // Cafe bounds are (7,0,26,4); (20,2) is comfortably inside.
        current_pos: (20, 2),
        target_pos: None,
        room_id: room.into(),
        status: "idle".into(),
    }
}

#[tokio::test]
async fn cross_startup_chat_in_cafe_delivered() {
    let (pool, layout, graph) = fixture().await;
    let mut w = WorldView::default();
    // Both avatars in cafe (public room) — simulating arrival post move_intent.
    w.avatars.insert(
        "alpha_eng".into(),
        av_in("alpha_eng", "alpha", "engineer", "cafe"),
    );
    w.avatars.insert(
        "beta_des".into(),
        av_in("beta_des", "beta", "designer", "cafe"),
    );

    let mut paths: PathStore = HashMap::new();
    let mut out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (tx_b, mut rx_b) = mpsc::channel(8);
    out_bus.insert("beta_des".into(), tx_b);

    // α-engineer says chat in cafe.
    let r = mcp_dispatch::dispatch(
        &mut w,
        &mut paths,
        &layout,
        &graph,
        &out_bus,
        &pool,
        "alpha_eng",
        json!({
            "type": "mcp_call", "v": 1, "tool": "speak", "corr_id": "c1",
            "args": { "kind": "chat", "body": "anyone tried mdast?" }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply", "speak chat: {r}");

    // β-designer's worker should receive the chat (despite being a different
    // startup) because cafe is a public room — invariant 7.
    let chat = rx_b
        .try_recv()
        .expect("beta_des should receive chat in cafe (public room invariant 7)");
    assert_eq!(chat["type"], "chat_received");
    assert_eq!(chat["from_agent_id"], "alpha_eng");
    assert_eq!(chat["body"], "anyone tried mdast?");
    assert_eq!(chat["room_id"], "cafe");
}

#[tokio::test]
async fn cross_startup_chat_in_suite_blocked() {
    let (pool, layout, graph) = fixture().await;
    let mut w = WorldView::default();
    // Both avatars in suite_1 (private to alpha in production, but we place
    // β-designer there directly to verify the chat filter — even if β were
    // somehow in α's suite, chat must NOT cross the startup boundary in a
    // private room.
    w.avatars.insert(
        "alpha_eng".into(),
        av_in("alpha_eng", "alpha", "engineer", "suite_1"),
    );
    w.avatars.insert(
        "beta_des".into(),
        av_in("beta_des", "beta", "designer", "suite_1"),
    );

    let mut paths: PathStore = HashMap::new();
    let mut out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (tx_b, mut rx_b) = mpsc::channel(8);
    out_bus.insert("beta_des".into(), tx_b);

    let r = mcp_dispatch::dispatch(
        &mut w,
        &mut paths,
        &layout,
        &graph,
        &out_bus,
        &pool,
        "alpha_eng",
        json!({
            "type": "mcp_call", "v": 1, "tool": "speak", "corr_id": "c1",
            "args": { "kind": "chat", "body": "secret" }
        }),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply");

    // β should NOT receive — same-startup-only rule applies in private rooms.
    let recv_result = rx_b.try_recv();
    assert!(
        recv_result.is_err(),
        "private-room chat should not cross startups; got {recv_result:?}"
    );
}

#[tokio::test]
async fn same_startup_chat_in_cafe_still_works() {
    // Sanity: same-startup chat in cafe is also delivered (no regression
    // from the cross-startup loosening).
    let (pool, layout, graph) = fixture().await;
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('alpha_des', 'alpha', 'AD', 'designer', 'claude_code', '', '{}', 'suite_1', 'idle')"
    ).execute(&pool).await.unwrap();

    let mut w = WorldView::default();
    w.avatars.insert(
        "alpha_eng".into(),
        av_in("alpha_eng", "alpha", "engineer", "cafe"),
    );
    w.avatars.insert(
        "alpha_des".into(),
        av_in("alpha_des", "alpha", "designer", "cafe"),
    );

    let mut paths: PathStore = HashMap::new();
    let mut out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (tx_d, mut rx_d) = mpsc::channel(8);
    out_bus.insert("alpha_des".into(), tx_d);

    let _ = mcp_dispatch::dispatch(
        &mut w,
        &mut paths,
        &layout,
        &graph,
        &out_bus,
        &pool,
        "alpha_eng",
        json!({
            "type": "mcp_call", "v": 1, "tool": "speak", "corr_id": "c1",
            "args": { "kind": "chat", "body": "hi team" }
        }),
    )
    .await;
    let chat = rx_d
        .try_recv()
        .expect("alpha_des should receive chat");
    assert_eq!(chat["type"], "chat_received");
    assert_eq!(chat["body"], "hi team");
}
