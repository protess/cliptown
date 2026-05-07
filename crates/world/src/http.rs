use axum::{
    extract::{ws::{WebSocket, WebSocketUpgrade, Message}, State},
    response::{Json, Response},
    routing::{get, post},
    Router,
};
use serde_json::json;
use std::sync::Arc;
use sqlx::SqlitePool;
use tokio::sync::oneshot;
use crate::loop_::{Cmd, Handle};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub handle: Handle,
    pub catalog: Arc<tokio::sync::RwLock<std::collections::HashMap<String, serde_json::Value>>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({"ok": true})) }))
        .route("/api/backend-catalog", get(api_catalog))
        .route("/api/backend-catalog/recheck", post(api_recheck))
        .route("/ws/console", get(ws_console))
        .route("/ws/worker", get(ws_worker))
        .with_state(Arc::new(state))
}

async fn api_catalog(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let m = s.catalog.read().await;
    Json(serde_json::to_value(&*m).unwrap())
}

async fn api_recheck(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let new_cat = crate::backend_catalog::probe_all().await;
    *s.catalog.write().await = new_cat.clone();
    let _ = s.handle.tx.send(Cmd::BackendCatalogUpdated(new_cat.clone())).await;
    Json(serde_json::json!({"ok": true, "entries": new_cat}))
}

async fn ws_console(ws: WebSocketUpgrade, State(s): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |sock| handle_console(sock, s))
}
async fn ws_worker(ws: WebSocketUpgrade, State(s): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |sock| handle_worker(sock, s))
}

async fn handle_console(mut socket: WebSocket, state: Arc<AppState>) {
    let Some(Ok(Message::Text(first))) = socket.recv().await else { return; };
    let parsed: serde_json::Value = match serde_json::from_str(&first) { Ok(v) => v, Err(_) => return };
    if parsed.get("type") != Some(&serde_json::Value::String("hello".into())) { return; }
    let token = parsed.get("operator_token").and_then(|v| v.as_str()).unwrap_or("");
    if crate::auth::validate_operator_token(&state.pool, token).await.is_err() {
        let _ = socket.send(Message::Text(r#"{"type":"auth_error"}"#.into())).await;
        return;
    }
    while let Some(Ok(Message::Text(txt))) = socket.recv().await {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&txt) else { continue; };
        let (tx, rx) = oneshot::channel();
        let _ = state.handle.tx.send(Cmd::HandleConsoleMsg { msg, reply: tx }).await;
        if let Ok(reply) = rx.await {
            let _ = socket.send(Message::Text(reply.to_string().into())).await;
        }
    }
}

async fn handle_worker(mut socket: WebSocket, state: Arc<AppState>) {
    let Some(Ok(Message::Text(first))) = socket.recv().await else { return; };
    let parsed: serde_json::Value = match serde_json::from_str(&first) { Ok(v) => v, Err(_) => return };
    if parsed.get("type") != Some(&serde_json::Value::String("hello".into())) { return; }
    let agent_id = parsed.get("agent_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let secret = parsed.get("secret").and_then(|v| v.as_str()).unwrap_or("");
    if crate::auth::validate_agent_secret(&state.pool, &agent_id, secret).await.is_err() {
        let _ = socket.send(Message::Text(r#"{"type":"auth_error"}"#.into())).await;
        return;
    }
    while let Some(Ok(Message::Text(txt))) = socket.recv().await {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&txt) else { continue; };
        let (tx, rx) = oneshot::channel();
        let _ = state.handle.tx.send(Cmd::HandleWorkerMsg { agent_id: agent_id.clone(), msg, reply: tx }).await;
        if let Ok(reply) = rx.await {
            let _ = socket.send(Message::Text(reply.to_string().into())).await;
        }
    }
}
