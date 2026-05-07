use cliptown_world::{http, loop_, state::WorldView, storage};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

async fn boot() -> std::net::SocketAddr {
    let dir = tempfile::tempdir().unwrap();
    let pool = storage::open(dir.path().join("test.db").to_str().unwrap()).await.unwrap();
    // insert a test agent so validate_agent_secret can find it
    sqlx::query("INSERT INTO towns (id, name, map_json) VALUES ('t', 'T', '{}')").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) VALUES ('s', 'S', 'g', 1.0, 't', '/tmp/s', 'active', 0)").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO rooms (id, town_id, name, type, bounds) VALUES ('r', 't', 'R', 'office', '{}')").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) VALUES ('a1', 's', 'A1', 'founder', 'claude_code', 'claude-3-5-sonnet', '{}', 'r', 'idle')").execute(&pool).await.unwrap();
    let handle = loop_::spawn(WorldView::default());
    let catalog = std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
    let app = http::router(http::AppState { pool, handle, catalog });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    // tempdir lives until end of scope; for tests this leaks a tempdir per test (acceptable in CI)
    std::mem::forget(dir);
    addr
}

#[tokio::test]
async fn console_rejects_bad_token() {
    let addr = boot().await;
    let url = format!("ws://{addr}/ws/console");
    let (mut s, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    s.send(Message::Text(r#"{"type":"hello","operator_token":"WRONG"}"#.into())).await.unwrap();
    let reply = s.next().await.unwrap().unwrap().into_text().unwrap();
    assert!(reply.contains("auth_error"), "expected auth_error, got {reply}");
}

#[tokio::test]
async fn worker_rejects_bad_secret() {
    let addr = boot().await;
    let url = format!("ws://{addr}/ws/worker");
    let (mut s, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    s.send(Message::Text(r#"{"type":"hello","agent_id":"a1","secret":"WRONG"}"#.into())).await.unwrap();
    let reply = s.next().await.unwrap().unwrap().into_text().unwrap();
    assert!(reply.contains("auth_error"));
}

#[tokio::test]
async fn worker_accepts_valid_secret() {
    let addr = boot().await;
    let url = format!("ws://{addr}/ws/worker");
    let (mut s, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    s.send(Message::Text(r#"{"type":"hello","agent_id":"a1","secret":"dev-secret"}"#.into())).await.unwrap();
    // After auth, send a no-op message and expect an {"ok":true} echo (M1.2 stub reply)
    s.send(Message::Text(r#"{"type":"ping"}"#.into())).await.unwrap();
    let reply = s.next().await.unwrap().unwrap().into_text().unwrap();
    assert!(reply.contains(r#""ok":true"#), "expected ok:true echo, got {reply}");
}
