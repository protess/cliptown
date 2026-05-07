use axum::{routing::get, Router, response::Json};
use serde_json::json;

pub fn router_minimal() -> Router {
    Router::new().route("/health", get(|| async { Json(json!({"ok": true})) }))
}
