//! Phase 3 carry-forward: admin-only task creation endpoint.
//!
//! Smoke harness + remote operators need a way to seed tasks without direct
//! SQL access. This endpoint mirrors what `tests/*.rs` currently do via raw
//! `INSERT INTO tasks` — but gated on a manager-or-above operator token and
//! validated against the existing world model (agents exist, startup exists,
//! same startup).
//!
//! Status: tasks created here land as `queued` when an `assignee_agent_id` is
//! supplied (the scheduler picks them up on the next tick) or `proposed`
//! when no assignee is set (an operator/manager later accepts the proposal).

use crate::auth::{validate_operator_token, OperatorRole};
use crate::http::AppState;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct CreateTaskBody {
    pub startup_id: String,
    pub title: String,
    pub description: String,
    pub assignee_agent_id: Option<String>,
    pub parent_id: Option<String>,
    pub required_room: Option<String>,
    pub preferred_backend: Option<String>,
    pub preferred_model: Option<String>,
}

pub async fn create_task(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateTaskBody>,
) -> Response {
    let tok = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or(Some(s)))
        .or_else(|| headers.get("x-operator-token").and_then(|v| v.to_str().ok()))
        .unwrap_or("");
    let identity = match validate_operator_token(&s.pool, tok).await {
        Ok(id) => id,
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, Json(json!({"error":"unauthorized"}))).into_response();
        }
    };
    if !identity.role.at_least(OperatorRole::Manager) {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"forbidden"}))).into_response();
    }
    if body.title.trim().is_empty() || body.description.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":"title and description required"}))).into_response();
    }

    // Verify the startup exists.
    let startup_row: Option<(String,)> = match sqlx::query_as(
        "SELECT id FROM startups WHERE id = ?",
    )
    .bind(&body.startup_id)
    .fetch_optional(&s.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":"sql","detail":e.to_string()}))).into_response();
        }
    };
    if startup_row.is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({"error":"unknown_startup"}))).into_response();
    }

    // If an assignee is set, verify it exists AND belongs to the same startup.
    if let Some(aid) = &body.assignee_agent_id {
        let agent_row: Option<(String,)> = match sqlx::query_as(
            "SELECT startup_id FROM agents WHERE id = ?",
        )
        .bind(aid)
        .fetch_optional(&s.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":"sql","detail":e.to_string()}))).into_response();
            }
        };
        match agent_row {
            None => return (StatusCode::NOT_FOUND, Json(json!({"error":"unknown_assignee"}))).into_response(),
            Some((sid,)) if sid != body.startup_id => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error":"cross_startup"}))).into_response();
            }
            _ => {}
        }
    }

    // queued when assignee set, proposed when not (mirrors create_subtask).
    let status = if body.assignee_agent_id.is_some() { "queued" } else { "proposed" };
    let task_id = format!("T_{}", Uuid::new_v4().simple());
    let r = sqlx::query(
        "INSERT INTO tasks (id, startup_id, parent_id, title, description, status, \
                            assignee_agent_id, required_room, preferred_backend, preferred_model, \
                            created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch(), unixepoch())",
    )
    .bind(&task_id)
    .bind(&body.startup_id)
    .bind(&body.parent_id)
    .bind(&body.title)
    .bind(&body.description)
    .bind(status)
    .bind(&body.assignee_agent_id)
    .bind(&body.required_room)
    .bind(&body.preferred_backend)
    .bind(&body.preferred_model)
    .execute(&s.pool)
    .await;
    match r {
        Ok(_) => {
            let _ = crate::persist::append_audit(
                &s.pool,
                &task_id,
                &json!({
                    "actor": "admin_api",
                    "kind": "task_created",
                    "operator_id": identity.id,
                })
                .to_string(),
            )
            .await;
            (
                StatusCode::CREATED,
                Json(json!({
                    "id": task_id,
                    "status": status,
                    "startup_id": body.startup_id,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"sql","detail":e.to_string()})),
        )
            .into_response(),
    }
}
