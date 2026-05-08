//! M6.1 — Invariant 8: directive isolation across startups.
//!
//! Verifies that two concurrent startups (`alpha`, `beta`) cannot see or
//! mutate each other's tasks, messages, directives, or artifacts. This is
//! a verification test that exercises the existing cross-startup gates
//! installed in M2.3 and the per-startup workspace sandbox from §6.3.

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
    std::mem::forget(dir);

    // Two startups α and β, each with founder + engineer.
    for (sid, suite) in &[("alpha", "suite_1"), ("beta", "suite_2")] {
        sqlx::query(
            "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
             VALUES (?, ?, 'g', 5.0, 'town_default', ?, 'active', unixepoch())"
        ).bind(sid).bind(sid).bind(format!("workspaces/{}", sid))
         .execute(&pool).await.unwrap();
        sqlx::query("UPDATE rooms SET private_to_startup_id = ? WHERE id = ?")
            .bind(sid).bind(suite).execute(&pool).await.unwrap();

        let founder = format!("{}_founder", sid);
        let engineer = format!("{}_engineer", sid);
        sqlx::query(
            "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status, manager_id) \
             VALUES (?, ?, 'F', 'founder', 'claude_code', '', '{}', ?, 'idle', NULL)"
        ).bind(&founder).bind(sid).bind(suite).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status, manager_id) \
             VALUES (?, ?, 'E', 'engineer', 'claude_code', '', '{}', ?, 'idle', ?)"
        ).bind(&engineer).bind(sid).bind(suite).bind(&founder).execute(&pool).await.unwrap();

        // Each startup has a parent task assigned to its founder.
        let parent_id = format!("parent_{}", sid);
        sqlx::query(
            "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
             VALUES (?, ?, 'parent', 'desc', 'in_progress', ?, unixepoch(), unixepoch())"
        ).bind(&parent_id).bind(sid).bind(&founder).execute(&pool).await.unwrap();
    }

    let mut layout = TownLayout::default_town();
    for r in layout.rooms.iter_mut() {
        if r.id == "suite_1" { r.private_to_startup_id = Some("alpha".into()); }
        if r.id == "suite_2" { r.private_to_startup_id = Some("beta".into()); }
    }
    let graph = move_sys::graph_from_layout(&layout);
    (pool, layout, graph)
}

fn av(id: &str, sid: &str, role: &str, room: &str) -> AvatarView {
    AvatarView {
        agent_id: id.into(),
        startup_id: sid.into(),
        role: role.into(),
        backend: "claude_code".into(),
        current_pos: (3, 3),
        target_pos: None,
        room_id: room.into(),
        status: "idle".into(),
    }
}

#[tokio::test]
async fn alpha_subtask_create_does_not_touch_beta() {
    let (pool, layout, graph) = fixture().await;
    let mut w = WorldView::default();
    w.avatars.insert("alpha_founder".into(),  av("alpha_founder", "alpha", "founder", "suite_1"));
    w.avatars.insert("alpha_engineer".into(), av("alpha_engineer", "alpha", "engineer", "suite_1"));
    w.avatars.insert("beta_founder".into(),   av("beta_founder", "beta", "founder", "suite_2"));
    w.avatars.insert("beta_engineer".into(),  av("beta_engineer", "beta", "engineer", "suite_2"));

    let mut paths: PathStore = HashMap::new();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();

    // α-founder creates a subtask under their own parent.
    let r = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, "alpha_founder",
        json!({
            "type": "mcp_call", "v": 1, "tool": "subtask_create", "corr_id": "c1",
            "args": {
                "parent_id": "parent_alpha",
                "title": "alpha task",
                "description": "alpha-only",
                "assignee_agent_id": "alpha_engineer",
                "required_room": null
            }
        })
    ).await;
    assert_eq!(r["type"], "mcp_reply");

    // β's task list count is unchanged (still just `parent_beta`).
    let beta_count: (i64,) = sqlx::query_as("SELECT count(*) FROM tasks WHERE startup_id = 'beta'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(beta_count.0, 1);

    // α has 2 (parent + new subtask).
    let alpha_count: (i64,) = sqlx::query_as("SELECT count(*) FROM tasks WHERE startup_id = 'alpha'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(alpha_count.0, 2);
}

#[tokio::test]
async fn alpha_directive_to_beta_engineer_rejected() {
    let (pool, layout, graph) = fixture().await;
    let mut w = WorldView::default();
    w.avatars.insert("alpha_founder".into(),  av("alpha_founder", "alpha", "founder", "suite_1"));
    w.avatars.insert("beta_engineer".into(),  av("beta_engineer", "beta", "engineer", "suite_2"));

    let mut paths: PathStore = HashMap::new();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();

    // α-founder tries to send a directive to β's engineer.
    let r = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, "alpha_founder",
        json!({
            "type": "mcp_call", "v": 1, "tool": "speak", "corr_id": "c1",
            "args": { "kind": "directive", "to_agent_id": "beta_engineer", "body": "do my bidding" }
        })
    ).await;
    assert_eq!(r["type"], "mcp_error");
    assert_eq!(r["code"], "cross_startup");

    // No message row was inserted.
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM messages").fetch_one(&pool).await.unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn alpha_task_done_on_beta_task_rejected() {
    let (pool, layout, graph) = fixture().await;
    let mut w = WorldView::default();
    w.avatars.insert("alpha_engineer".into(), av("alpha_engineer", "alpha", "engineer", "suite_1"));
    let mut paths: PathStore = HashMap::new();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();

    // α-engineer tries to mark β's parent task done.
    let r = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, "alpha_engineer",
        json!({
            "type": "mcp_call", "v": 1, "tool": "task_done", "corr_id": "c1",
            "args": { "task_id": "parent_beta", "artifact_path": "workspaces/alpha/artifacts/parent_beta.md" }
        })
    ).await;
    assert_eq!(r["type"], "mcp_error");

    // β's parent task untouched.
    let row: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id = 'parent_beta'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(row.0, "in_progress");
}

#[tokio::test]
async fn alpha_read_artifact_with_beta_path_rejected() {
    let (pool, layout, graph) = fixture().await;
    let mut w = WorldView::default();
    w.avatars.insert("alpha_engineer".into(), av("alpha_engineer", "alpha", "engineer", "suite_1"));
    let mut paths: PathStore = HashMap::new();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();

    // Materialize a fake artifact in beta's space.
    let beta_artifacts = std::env::current_dir().unwrap().join("workspaces").join("beta").join("artifacts");
    tokio::fs::create_dir_all(&beta_artifacts).await.ok();
    tokio::fs::write(beta_artifacts.join("secret.md"), "trade secrets").await.ok();

    // α-engineer's sandbox is rooted at workspaces/alpha. Try to escape via ".." to beta.
    let r = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, "alpha_engineer",
        json!({
            "type": "mcp_call", "v": 1, "tool": "read_artifact", "corr_id": "c1",
            "args": { "path": "../beta/artifacts/secret.md" }
        })
    ).await;
    assert_eq!(r["type"], "mcp_error");
    // Sandbox rejects path escape.
    let _ = tokio::fs::remove_dir_all(std::env::current_dir().unwrap().join("workspaces").join("beta")).await;
}
