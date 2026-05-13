//! P2.2 MCP-level skills tests — round-trip through dispatch using a real
//! caller AvatarView. Skips the WS/HTTP outer layers and calls dispatch
//! directly with synthetic mcp_tool_use payloads.

use cliptown_world::mcp_dispatch;
use cliptown_world::state::{AvatarView, WorldView};
use cliptown_world::storage;
use serde_json::json;
use sqlx::SqlitePool;
use std::collections::HashMap;
use tokio::sync::{broadcast, mpsc};

async fn ctx() -> (SqlitePool, AvatarView) {
    let dir = tempfile::tempdir().unwrap();
    let pool = storage::open(dir.path().join("t.db").to_str().unwrap())
        .await
        .unwrap();
    cliptown_world::seed::seed_if_empty(&pool).await.unwrap();
    sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, budget_spent_usd, town_id, workspace_path, status, created_at) VALUES ('S1','alpha','g',10.0,0.0,'town_default','/tmp/s1','active',unixepoch())").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, manager_id, status) VALUES ('A1','S1','eng','engineer','claude_code','m','{\"x\":0,\"y\":0}','lobby',NULL,'idle')").execute(&pool).await.unwrap();
    std::mem::forget(dir);
    let caller = AvatarView {
        agent_id: "A1".to_string(),
        startup_id: "S1".to_string(),
        role: "engineer".to_string(),
        backend: "claude_code".to_string(),
        current_pos: (0, 0),
        target_pos: None,
        room_id: "lobby".to_string(),
        status: "idle".to_string(),
        last_seen_at: None,
        health: cliptown_world::health::Health::Offline,
    };
    (pool, caller)
}

async fn dispatch(
    pool: &SqlitePool,
    caller: &AvatarView,
    tool: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    let mut w = WorldView::default();
    w.avatars.insert(caller.agent_id.clone(), caller.clone());
    let mut paths = HashMap::new();
    let layout = cliptown_world::seed::TownLayout::default_town();
    let graph = cliptown_world::move_sys::graph_from_layout(&layout);
    let out_bus: HashMap<String, mpsc::Sender<serde_json::Value>> = HashMap::new();
    let (event_tx, _event_rx) = broadcast::channel(8);
    let msg = json!({
        "type": "mcp_tool_use",
        "v": 1,
        "corr_id": "c1",
        "tool": tool,
        "args": args,
    });
    mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &out_bus, pool, &event_tx,
        &caller.agent_id, msg,
    )
    .await
}

#[tokio::test]
async fn mcp_skill_upsert_then_list_round_trip() {
    let (pool, caller) = ctx().await;
    let r = dispatch(
        &pool,
        &caller,
        "skill_upsert",
        json!({"name":"deploy","content_md":"hello"}),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply");
    assert_eq!(r["result"]["created"], true);
    let id = r["result"]["id"].as_str().unwrap().to_string();
    assert!(!id.is_empty());

    let l = dispatch(&pool, &caller, "skill_list", json!({})).await;
    assert_eq!(l["type"], "mcp_reply");
    let items = l["result"]["skills"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "deploy");
    assert_eq!(items[0]["len"], 5);
}

#[tokio::test]
async fn mcp_skill_attach_then_detach() {
    let (pool, caller) = ctx().await;
    let u = dispatch(
        &pool,
        &caller,
        "skill_upsert",
        json!({"name":"deploy","content_md":"hello"}),
    )
    .await;
    let sid = u["result"]["id"].as_str().unwrap().to_string();
    let a = dispatch(
        &pool,
        &caller,
        "skill_attach",
        json!({"agent_id":"A1","skill_id":sid}),
    )
    .await;
    assert_eq!(a["type"], "mcp_reply");
    let count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM agent_skills WHERE agent_id = 'A1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 1);

    let d = dispatch(
        &pool,
        &caller,
        "skill_detach",
        json!({"agent_id":"A1","skill_id":sid}),
    )
    .await;
    assert_eq!(d["type"], "mcp_reply");
    let count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM agent_skills WHERE agent_id = 'A1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn mcp_skill_delete_cascades_attachment() {
    let (pool, caller) = ctx().await;
    let u = dispatch(
        &pool,
        &caller,
        "skill_upsert",
        json!({"name":"deploy","content_md":"hello"}),
    )
    .await;
    let sid = u["result"]["id"].as_str().unwrap().to_string();
    dispatch(
        &pool,
        &caller,
        "skill_attach",
        json!({"agent_id":"A1","skill_id":sid}),
    )
    .await;
    let d = dispatch(&pool, &caller, "skill_delete", json!({"skill_id":sid})).await;
    assert_eq!(d["type"], "mcp_reply");
    let count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM agent_skills WHERE agent_id = 'A1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 0);
}
