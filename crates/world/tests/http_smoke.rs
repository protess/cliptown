use axum::body::to_bytes;
use cliptown_world::{
    agent_supervisor::{AgentSupervisor, SupervisorConfig},
    http, loop_, state::WorldView, storage,
};
use std::sync::Arc;
use tower::ServiceExt;

mod common;
use common::TestCtx;

#[tokio::test]
async fn health_returns_ok_json() {
    let dir = tempfile::tempdir().unwrap();
    let pool = storage::open(dir.path().join("test.db").to_str().unwrap()).await.unwrap();
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(64);
    let handle = loop_::spawn(WorldView::default(), pool.clone(), event_tx.clone());
    let catalog = std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
    let supervisor = Arc::new(AgentSupervisor::new(SupervisorConfig::default(), pool.clone(), event_tx.clone()));
    let app = http::router(http::AppState { pool, handle, catalog, supervisor, max_review_rounds: 3 });
    let req = axum::http::Request::builder().uri("/health").body(axum::body::Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], br#"{"ok":true}"#);
}

#[tokio::test]
async fn snapshot_includes_review_round_and_max() {
    let ctx = TestCtx::new().await;
    sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) VALUES ('s1','a','g',10.0,'town_default','/tmp','active',unixepoch())").execute(&ctx.pool).await.unwrap();
    sqlx::query("INSERT INTO tasks (id, startup_id, title, description, status, review_round, created_at, updated_at) VALUES ('T1', 's1', 't', 'd', 'in_progress', 2, unixepoch(), unixepoch())").execute(&ctx.pool).await.unwrap();

    let view = cliptown_world::state::WorldView::default();
    let frame = http::build_console_snapshot(&ctx.pool, &view, 3 /* max */).await;
    let tasks = frame["snapshot"]["tasks"].as_array().expect("snapshot.tasks should be an array");
    let t1 = tasks.iter().find(|t| t["id"] == "T1").expect("T1 should be in snapshot");
    assert_eq!(t1["review_round"], 2);
    assert_eq!(t1["max_review_rounds"], 3);
}

/// Theme G slice 2: the snapshot's `startups[*]` entries surface
/// `auto_steal_enabled` + `auto_steal_after_secs` so the admin-only
/// MainHeader popover hydrates without a side fetch.
#[tokio::test]
async fn snapshot_startups_surface_auto_steal_fields() {
    let ctx = TestCtx::new().await;
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at, auto_steal_enabled, auto_steal_after_secs) \
         VALUES ('s1','a','g',10.0,'town_default','/tmp','active',unixepoch(), 1, 30)"
    ).execute(&ctx.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s2','b','g',10.0,'town_default','/tmp','active',unixepoch())"
    ).execute(&ctx.pool).await.unwrap();

    let view = cliptown_world::state::WorldView::default();
    let frame = http::build_console_snapshot(&ctx.pool, &view, 3).await;
    let startups = frame["snapshot"]["startups"].as_array().unwrap();
    let s1 = startups.iter().find(|s| s["id"] == "s1").unwrap();
    let s2 = startups.iter().find(|s| s["id"] == "s2").unwrap();
    assert_eq!(s1["auto_steal_enabled"], true);
    assert_eq!(s1["auto_steal_after_secs"], 30);
    assert_eq!(s2["auto_steal_enabled"], false, "default-off must surface as false");
    assert_eq!(s2["auto_steal_after_secs"], 60, "SQL default 60s must surface");
}

/// P6 Theme C: the snapshot surfaces per-startup auto-recovery flag +
/// max_attempts so the admin-only MainHeader toggle hydrates without a
/// side fetch.
#[tokio::test]
async fn snapshot_startups_surface_auto_recovery_fields() {
    let ctx = TestCtx::new().await;
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at, auto_recovery_enabled, auto_recovery_max_attempts) \
         VALUES ('s1','a','g',10.0,'town_default','/tmp','active',unixepoch(), 1, 3)"
    ).execute(&ctx.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s2','b','g',10.0,'town_default','/tmp','active',unixepoch())"
    ).execute(&ctx.pool).await.unwrap();

    let view = cliptown_world::state::WorldView::default();
    let frame = http::build_console_snapshot(&ctx.pool, &view, 3).await;
    let startups = frame["snapshot"]["startups"].as_array().unwrap();
    let s1 = startups.iter().find(|s| s["id"] == "s1").unwrap();
    let s2 = startups.iter().find(|s| s["id"] == "s2").unwrap();
    assert_eq!(s1["auto_recovery_enabled"], true);
    assert_eq!(s1["auto_recovery_max_attempts"], 3);
    assert_eq!(s2["auto_recovery_enabled"], false, "default-off must surface as false");
    assert_eq!(s2["auto_recovery_max_attempts"], 2, "SQL default 2 must surface");
}

/// Theme G slice 3: each task object carries `blocked_on` + `deadline_at`
/// so Kanban cards can render the badges from E2 without a side fetch.
#[tokio::test]
async fn snapshot_tasks_surface_blocked_on_and_deadline_at() {
    let ctx = TestCtx::new().await;
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1','a','g',10.0,'town_default','/tmp','active',unixepoch())"
    ).execute(&ctx.pool).await.unwrap();
    let due = chrono::Utc::now().timestamp() + 3600;
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, blocked_on, deadline_at, created_at, updated_at) \
         VALUES ('T_block', 's1', 'blocker', 'd', 'queued', NULL, NULL, unixepoch(), unixepoch())"
    ).execute(&ctx.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, blocked_on, deadline_at, created_at, updated_at) \
         VALUES ('T_held', 's1', 'held', 'd', 'queued', 'T_block', ?, unixepoch(), unixepoch())"
    ).bind(due).execute(&ctx.pool).await.unwrap();

    let view = cliptown_world::state::WorldView::default();
    let frame = http::build_console_snapshot(&ctx.pool, &view, 3).await;
    let tasks = frame["snapshot"]["tasks"].as_array().unwrap();
    let held = tasks.iter().find(|t| t["id"] == "T_held").unwrap();
    assert_eq!(held["blocked_on"], "T_block");
    assert_eq!(held["deadline_at"], due);
    let blocker = tasks.iter().find(|t| t["id"] == "T_block").unwrap();
    assert!(blocker["blocked_on"].is_null());
    assert!(blocker["deadline_at"].is_null());
}

/// Theme G slice 2: each avatar object carries `is_peer_reviewer` so the
/// AgentsPanel can render the per-agent checkbox without a side fetch.
#[tokio::test]
async fn snapshot_avatars_surface_is_peer_reviewer() {
    use cliptown_world::state::{AvatarView, WorldView};
    let ctx = TestCtx::new().await;
    sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) VALUES ('s1','a','g',10.0,'town_default','/tmp','active',unixepoch())").execute(&ctx.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status, is_peer_reviewer) \
         VALUES ('e1', 's1', 'E1', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle', 1)"
    ).execute(&ctx.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('e2', 's1', 'E2', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle')"
    ).execute(&ctx.pool).await.unwrap();

    let mut view = WorldView::default();
    for (id, pr_role) in [("e1", "engineer"), ("e2", "engineer")] {
        view.avatars.insert(id.into(), AvatarView {
            agent_id: id.into(), startup_id: "s1".into(), role: pr_role.into(),
            backend: "claude_code".into(), current_pos: (0,0), target_pos: None,
            room_id: "suite_1".into(), status: "idle".into(), last_seen_at: None,
            health: cliptown_world::health::Health::Online,
        });
    }
    let frame = http::build_console_snapshot(&ctx.pool, &view, 3).await;
    let avatars = frame["snapshot"]["avatars"].as_array().unwrap();
    let a1 = avatars.iter().find(|a| a["agent_id"] == "e1").unwrap();
    let a2 = avatars.iter().find(|a| a["agent_id"] == "e2").unwrap();
    assert_eq!(a1["is_peer_reviewer"], true);
    assert_eq!(a2["is_peer_reviewer"], false, "default-off avatars must surface as false");
}
