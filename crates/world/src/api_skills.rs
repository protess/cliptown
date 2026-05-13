//! P2.2 HTTP endpoint for the worker to fetch an agent's attached skills.
//! Bearer auth via `<agent_id>:<secret>` (same scheme as MCP HTTP). The
//! caller must equal `:agent_id` in the path.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

use crate::http::AppState;

pub async fn get_agent_skills(
    Path(agent_id): Path<String>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let (claimed_agent, secret) = match token.split_once(':') {
        Some((a, s)) => (a, s),
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "bad_auth_format"})),
            )
                .into_response();
        }
    };
    // The claimed agent must match the path agent — an agent can only fetch
    // its own skills.
    if claimed_agent != agent_id {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "agent_id mismatch"})),
        )
            .into_response();
    }
    // Validate via the shared auth helper (env-var-backed; default
    // "dev-secret" for tests/smoke).
    if crate::auth::validate_agent_secret(&state.pool, claimed_agent, secret)
        .await
        .is_err()
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_agent_secret"})),
        )
            .into_response();
    }
    let items = match crate::skills::for_agent(&state.pool, &agent_id).await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("{e:?}")})),
            )
                .into_response();
        }
    };
    let json_items: Vec<serde_json::Value> = items
        .into_iter()
        .map(|s| json!({"name": s.name, "content_md": s.content_md}))
        .collect();
    (StatusCode::OK, Json(json!({"skills": json_items}))).into_response()
}
