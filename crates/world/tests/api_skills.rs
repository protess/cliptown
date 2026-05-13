//! P2.2 HTTP endpoint tests — boot the axum router + send real requests.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use cliptown_world::{
    agent_supervisor::{AgentSupervisor, SupervisorConfig},
    http::{router, AppState},
    loop_, seed,
    state::WorldView,
    storage,
};
use std::sync::Arc;
use tower::ServiceExt;

async fn fixture() -> AppState {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("test.db");
    let pool = storage::open(p.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, budget_spent_usd, town_id, workspace_path, status, created_at) \
         VALUES ('S1','alpha','g',10.0,0.0,'town_default','/tmp/s1','active',unixepoch())",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, manager_id, status) \
         VALUES ('A1','S1','eng','engineer','claude_code','m','{\"x\":0,\"y\":0}','lobby',NULL,'idle')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let (sid, _) = cliptown_world::skills::upsert(&pool, "S1", "deploy", "hello")
        .await
        .unwrap();
    cliptown_world::skills::attach(&pool, "S1", "A1", &sid)
        .await
        .unwrap();

    // The fixture leak matches the convention in the other AppState tests
    // (see `tests/ws_auth.rs`): the pool keeps a ref to the file, and the
    // tempdir cleanup would race with that on drop.
    std::mem::forget(dir);

    let cargo_dir = env!("CARGO_MANIFEST_DIR");
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(64);
    let supervisor = Arc::new(AgentSupervisor::new(
        SupervisorConfig {
            worker_bin: "/bin/sh".into(),
            worker_args: vec![format!("{}/tests/fixtures/fake_worker_long_run.sh", cargo_dir)],
            backoff_ms: vec![10, 20, 30],
            dissolve_grace_ms: 100,
        },
        pool.clone(),
        event_tx.clone(),
    ));

    let handle = loop_::spawn(WorldView::default(), pool.clone(), event_tx.clone());
    AppState {
        pool,
        handle,
        catalog: Arc::new(tokio::sync::RwLock::new(Default::default())),
        supervisor,
        max_review_rounds: 3,
    }
}

#[tokio::test]
async fn get_agent_skills_returns_attached_with_content() {
    let state = fixture().await;
    let app = router(state);
    let req = Request::builder()
        .method("GET")
        .uri("/api/agents/A1/skills")
        .header("Authorization", "Bearer A1:dev-secret")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = v["skills"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "deploy");
    assert_eq!(items[0]["content_md"], "hello");
}

#[tokio::test]
async fn get_agent_skills_rejects_wrong_bearer() {
    let state = fixture().await;
    let app = router(state);
    let req = Request::builder()
        .method("GET")
        .uri("/api/agents/A1/skills")
        .header("Authorization", "Bearer A1:wrong")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_agent_skills_rejects_mismatched_path_id() {
    let state = fixture().await;
    let app = router(state);
    // Agent A1's token used to fetch A2's skills (even if A2 doesn't exist).
    let req = Request::builder()
        .method("GET")
        .uri("/api/agents/A2/skills")
        .header("Authorization", "Bearer A1:dev-secret")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
