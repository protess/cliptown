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
