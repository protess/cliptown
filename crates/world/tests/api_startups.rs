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

/// Build an `AppState` wired to a fresh seeded SQLite + a supervisor whose
/// "worker binary" is the long-run shell fixture, so spawned children stay
/// alive for the duration of the assertion. Mirrors the pattern used by
/// `crates/world/tests/agent_supervisor.rs`.
async fn fixture() -> AppState {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("test.db");
    let pool = storage::open(p.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    // The fixture leak matches the convention in the other AppState tests
    // (see `tests/ws_auth.rs`): the pool keeps a ref to the file, and the
    // tempdir cleanup would race with that on drop.
    std::mem::forget(dir);

    let cargo_dir = env!("CARGO_MANIFEST_DIR");
    let supervisor = Arc::new(AgentSupervisor::new(
        SupervisorConfig {
            worker_bin: "/bin/sh".into(),
            worker_args: vec![format!("{}/tests/fixtures/fake_worker_long_run.sh", cargo_dir)],
            backoff_ms: vec![10, 20, 30],
            dissolve_grace_ms: 100,
        },
        pool.clone(),
    ));

    let handle = loop_::spawn(WorldView::default(), pool.clone());
    AppState {
        pool,
        handle,
        catalog: Arc::new(tokio::sync::RwLock::new(Default::default())),
        supervisor,
    }
}

#[tokio::test]
async fn post_creates_startup_and_3_agents() {
    let state = fixture().await;
    let app = router(state.clone());
    let body = serde_json::json!({
        "name": "alpha",
        "goal_text": "build something",
        "budget_cap_usd": 10.0,
        "backends": { "founder": "claude_code", "engineer": "claude_code", "designer": "claude_code" }
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/startups")
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = to_bytes(res.into_body(), 4096).await.unwrap();
    let resp_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let startup_id = resp_json.get("id").and_then(|v| v.as_str()).unwrap().to_string();
    assert_eq!(resp_json.get("agents").and_then(|v| v.as_array()).map(|a| a.len()), Some(3));

    // 1 startup + 3 agents persisted.
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM startups")
        .fetch_one(&state.pool).await.unwrap();
    assert_eq!(count.0, 1);
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM agents WHERE startup_id = ?")
        .bind(&startup_id).fetch_one(&state.pool).await.unwrap();
    assert_eq!(count.0, 3);

    // Manager chain: founder.manager_id IS NULL, the other two point at founder.
    let null_mgr: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM agents WHERE startup_id = ? AND role = 'founder' AND manager_id IS NULL"
    ).bind(&startup_id).fetch_one(&state.pool).await.unwrap();
    assert_eq!(null_mgr.0, 1);
    let founder_id: (String,) = sqlx::query_as(
        "SELECT id FROM agents WHERE startup_id = ? AND role = 'founder'"
    ).bind(&startup_id).fetch_one(&state.pool).await.unwrap();
    let mgr_match: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM agents WHERE startup_id = ? AND role IN ('engineer','designer') AND manager_id = ?"
    ).bind(&startup_id).bind(&founder_id.0).fetch_one(&state.pool).await.unwrap();
    assert_eq!(mgr_match.0, 2);

    // Suite was claimed.
    let claimed: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM rooms WHERE private_to_startup_id = ?"
    ).bind(&startup_id).fetch_one(&state.pool).await.unwrap();
    assert_eq!(claimed.0, 1);

    // Workspace artifacts dir exists.
    let workspace: (String,) = sqlx::query_as(
        "SELECT workspace_path FROM startups WHERE id = ?"
    ).bind(&startup_id).fetch_one(&state.pool).await.unwrap();
    let artifacts = std::path::Path::new(&workspace.0).join("artifacts");
    assert!(artifacts.is_dir(), "expected {} to exist", artifacts.display());
    // Cleanup so re-runs of the test don't leave junk in $CWD.
    let _ = std::fs::remove_dir_all(&workspace.0);

    // 3 workers spawned and tracked by the supervisor (the long-run fixture
    // sleeps 30s so they're still alive when we check).
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(state.supervisor.agent_count().await, 3);
}

#[tokio::test]
async fn post_409_when_no_free_suite() {
    let state = fixture().await;
    // Mark all suites taken.
    sqlx::query("UPDATE rooms SET private_to_startup_id = 'placeholder' WHERE type = 'office'")
        .execute(&state.pool).await.unwrap();
    let app = router(state.clone());
    let body = serde_json::json!({
        "name": "alpha", "goal_text": "x", "budget_cap_usd": 10.0,
        "backends": { "founder": "claude_code", "engineer": "claude_code", "designer": "claude_code" }
    });
    let req = Request::builder()
        .method("POST").uri("/api/startups")
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string())).unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn post_400_on_invalid_backend() {
    let state = fixture().await;
    let app = router(state.clone());
    let body = serde_json::json!({
        "name": "alpha", "goal_text": "x", "budget_cap_usd": 10.0,
        "backends": { "founder": "gpt4", "engineer": "claude_code", "designer": "claude_code" }
    });
    let req = Request::builder()
        .method("POST").uri("/api/startups")
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string())).unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}
