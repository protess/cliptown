use axum::body::to_bytes;
use cliptown_world::{
    agent_supervisor::{AgentSupervisor, SupervisorConfig},
    http, loop_, state::WorldView, storage,
};
use std::sync::Arc;
use tower::ServiceExt;

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
