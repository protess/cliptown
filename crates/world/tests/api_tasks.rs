//! Integration tests for the admin-only task creation endpoint
//! (`POST /api/admin/tasks`).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cliptown_world::http::AppState;
use cliptown_world::loop_;
use cliptown_world::state::WorldView;
use cliptown_world::{agent_supervisor, http, storage};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

async fn boot() -> (axum::Router, sqlx::SqlitePool) {
    let dir = tempfile::tempdir().unwrap();
    let pool = storage::open(dir.path().join("t.db").to_str().unwrap())
        .await
        .unwrap();
    cliptown_world::seed::seed_if_empty(&pool).await.unwrap();
    // Seed a startup + an agent so the endpoint's validation paths have
    // something to point at.
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'alpha', 'g', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('a1', 's1', 'A1', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle')"
    ).execute(&pool).await.unwrap();
    let (event_tx, _) = tokio::sync::broadcast::channel(64);
    let handle = loop_::spawn(WorldView::default(), pool.clone(), event_tx.clone());
    let supervisor = Arc::new(agent_supervisor::AgentSupervisor::new(
        agent_supervisor::SupervisorConfig::default(),
        pool.clone(),
        event_tx,
    ));
    let app = http::router(AppState {
        pool: pool.clone(),
        handle,
        catalog: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        supervisor,
        max_review_rounds: 3,
    });
    std::mem::forget(dir);
    (app, pool)
}

async fn post(app: axum::Router, body: Value, token: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/admin/tasks")
        .header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = req.body(Body::from(body.to_string())).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

#[tokio::test]
async fn rejects_missing_auth() {
    let (app, _pool) = boot().await;
    let (status, _) = post(
        app,
        json!({"startup_id":"s1","title":"t","description":"d","assignee_agent_id":"a1"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rejects_unknown_token() {
    let (app, _pool) = boot().await;
    let (status, _) = post(
        app,
        json!({"startup_id":"s1","title":"t","description":"d","assignee_agent_id":"a1"}),
        Some("not-a-real-token"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rejects_viewer_role() {
    let (app, pool) = boot().await;
    sqlx::query("INSERT INTO operators (id, name, token, role, created_at) VALUES ('op_v','viewer','tok_v','viewer',unixepoch())")
        .execute(&pool).await.unwrap();
    let (status, _) = post(
        app,
        json!({"startup_id":"s1","title":"t","description":"d","assignee_agent_id":"a1"}),
        Some("tok_v"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn creates_task_queued_when_assignee_set() {
    let (app, pool) = boot().await;
    let (status, body) = post(
        app,
        json!({
            "startup_id":"s1",
            "title":"haiku",
            "description":"write a 3-line haiku",
            "assignee_agent_id":"a1",
            "preferred_backend":"codex",
            "preferred_model":"gpt-5-mini"
        }),
        Some("dev-token"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["status"], "queued");
    let id = body["id"].as_str().unwrap();
    let row: (String, String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT status, title, preferred_backend, preferred_model FROM tasks WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "queued");
    assert_eq!(row.1, "haiku");
    assert_eq!(row.2.as_deref(), Some("codex"));
    assert_eq!(row.3.as_deref(), Some("gpt-5-mini"));
}

#[tokio::test]
async fn creates_task_proposed_without_assignee() {
    let (app, _pool) = boot().await;
    let (status, body) = post(
        app,
        json!({"startup_id":"s1","title":"t","description":"d"}),
        Some("dev-token"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["status"], "proposed");
}

#[tokio::test]
async fn rejects_unknown_startup() {
    let (app, _pool) = boot().await;
    let (status, _) = post(
        app,
        json!({"startup_id":"s_ghost","title":"t","description":"d"}),
        Some("dev-token"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rejects_cross_startup_assignee() {
    let (app, pool) = boot().await;
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s2', 'beta', 'g', 10.0, 'town_default', '/tmp/s2', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('a2', 's2', 'A2', 'engineer', 'claude_code', 'm', '{}', 'suite_3', 'idle')"
    ).execute(&pool).await.unwrap();
    let (status, _) = post(
        app,
        json!({"startup_id":"s1","title":"t","description":"d","assignee_agent_id":"a2"}),
        Some("dev-token"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
