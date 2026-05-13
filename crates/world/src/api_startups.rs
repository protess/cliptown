//! POST /api/startups — claim free suite, create startup + 3 agents, spawn workers.
//!
//! Phase 0 quirks
//! --------------
//! - **Per-agent secrets** are generated and stored in the **process environment**
//!   under `CLIPTOWN_AGENT_SECRET_<agent_id>` so `auth::validate_agent_secret`
//!   (see `crates/world/src/auth.rs`) finds them. Worker children inherit env
//!   from the world process, so they pick up the same secret on spawn. This is
//!   single-process by design; multi-host operation is post-Phase 0.
//! - **Worker binary** is configurable via `SupervisorConfig` (driven by
//!   `CLIPTOWN_WORKER_BIN` for the prod path). Tests set it to a fixture.
//! - **Suite slots**: there are exactly 4 seeded suites. Once exhausted the
//!   handler returns 409. Full waiting/queueing handling is in M5.2.
//!
//! Manager chain: `founder.manager_id = NULL`, both engineer and designer
//! point at the founder. This is enforced by the per-agent insert ordering
//! below — founder first so its id can be referenced by the others.

use crate::agent_supervisor::{per_task_workers_enabled, SpawnConfig};
use crate::health::Health;
use crate::http::AppState;
use crate::loop_::Cmd;
use crate::state::AvatarView;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

/// Pull the operator token from either `Authorization: Bearer <tok>` or the
/// bare `X-Operator-Token: <tok>` header. Mirrors `http::patch_startup` so the
/// PATCH and DELETE handlers agree on which auth headers they accept.
fn extract_operator_token(headers: &HeaderMap) -> &str {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or(Some(s)))
        .or_else(|| headers.get("x-operator-token").and_then(|v| v.to_str().ok()))
        .unwrap_or("")
}

#[derive(Debug, Deserialize)]
pub struct CreateStartupRequest {
    pub name: String,
    pub goal_text: String,
    pub budget_cap_usd: f64,
    pub backends: BackendChoice,
}

#[derive(Debug, Deserialize)]
pub struct BackendChoice {
    pub founder: String,
    pub engineer: String,
    pub designer: String,
}

#[derive(Debug, Serialize)]
pub struct CreateStartupResponse {
    pub id: String,
    pub suite_id: String,
    pub agents: Vec<AgentRef>,
}

#[derive(Debug, Serialize)]
pub struct AgentRef {
    pub id: String,
    pub role: String,
    pub backend: String,
}

/// Per-role tile offset *inside* the suite. Founder gets the front-left desk,
/// engineer the front-mid, designer the front-right. Suites are 7x6, so these
/// offsets are well within bounds for every seeded suite.
const HOME_DESK_OFFSETS: &[(&str, i32, i32)] = &[
    ("founder", 1, 1),
    ("engineer", 3, 1),
    ("designer", 5, 1),
];

/// Backends accepted by the `agents.backend` CHECK constraint.
const ALLOWED_BACKENDS: &[&str] = &["claude_code", "codex", "opencode"];

pub async fn create_startup(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateStartupRequest>,
) -> Response {
    // Operator auth — same shape as `delete_startup` / `http::patch_startup`.
    // Validate BEFORE the budget/backend checks so unauthenticated callers
    // can't probe for which fields the server happens to validate first
    // (and so a missing token never costs a suite slot or DB write).
    let tok = extract_operator_token(&headers);
    if crate::auth::validate_operator_token(&s.pool, tok).await.is_err() {
        return reply_err(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    // Validate budget. Mirrors the bounds used by `patch_startup` so the
    // create/update paths agree on what's acceptable.
    if !(req.budget_cap_usd.is_finite() && req.budget_cap_usd >= 0.0 && req.budget_cap_usd < 1_000_000.0) {
        return reply_err(StatusCode::BAD_REQUEST, "invalid budget_cap_usd");
    }
    if req.name.trim().is_empty() {
        return reply_err(StatusCode::BAD_REQUEST, "name required");
    }
    for b in [&req.backends.founder, &req.backends.engineer, &req.backends.designer] {
        if !ALLOWED_BACKENDS.contains(&b.as_str()) {
            return reply_err(StatusCode::BAD_REQUEST, "unknown backend");
        }
    }

    let mut tx = match s.pool.begin().await {
        Ok(t) => t,
        Err(e) => return reply_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    // 1. Claim a free suite. SELECT under the same tx as the UPDATE+INSERT
    //    keeps the claim atomic — two concurrent creates won't grab the same
    //    suite because SQLite serializes writes via its single-writer model.
    let free_suite: Option<(String, String)> = match sqlx::query_as(
        "SELECT id, bounds FROM rooms \
         WHERE type = 'office' AND private_to_startup_id IS NULL \
         ORDER BY id LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(v) => v,
        Err(e) => return reply_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    let (suite_id, suite_bounds_json) = match free_suite {
        Some(v) => v,
        None => return reply_err(StatusCode::CONFLICT, "no free suite"),
    };

    // Parse bounds {"x":..,"y":..,"w":..,"h":..} so we can position each agent
    // at a per-role offset inside the suite.
    let bounds: serde_json::Value = match serde_json::from_str(&suite_bounds_json) {
        Ok(v) => v,
        Err(e) => return reply_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("bounds parse: {e}")),
    };
    let suite_x = bounds.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let suite_y = bounds.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

    let startup_id = Uuid::new_v4().to_string();
    let workspace_path = format!("workspaces/{}", startup_id);

    // 2. Insert startup row.
    let r = sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES (?, ?, ?, ?, 'town_default', ?, 'active', unixepoch())",
    )
    .bind(&startup_id)
    .bind(&req.name)
    .bind(&req.goal_text)
    .bind(req.budget_cap_usd)
    .bind(&workspace_path)
    .execute(&mut *tx)
    .await;
    if let Err(e) = r {
        return reply_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }

    // Claim the suite.
    let r = sqlx::query("UPDATE rooms SET private_to_startup_id = ? WHERE id = ?")
        .bind(&startup_id)
        .bind(&suite_id)
        .execute(&mut *tx)
        .await;
    if let Err(e) = r {
        return reply_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }

    // 3. Insert 3 agent rows. Founder first so its id is available as
    //    manager_id for the engineer + designer rows.
    let founder_id = format!("a-{}", Uuid::new_v4().simple());
    let engineer_id = format!("a-{}", Uuid::new_v4().simple());
    let designer_id = format!("a-{}", Uuid::new_v4().simple());

    let agents: [(&str, &str, &str, Option<&str>); 3] = [
        ("founder", founder_id.as_str(), req.backends.founder.as_str(), None),
        ("engineer", engineer_id.as_str(), req.backends.engineer.as_str(), Some(founder_id.as_str())),
        ("designer", designer_id.as_str(), req.backends.designer.as_str(), Some(founder_id.as_str())),
    ];

    for (role, aid, backend, manager_id) in &agents {
        let (ox, oy) = HOME_DESK_OFFSETS
            .iter()
            .find(|(r, _, _)| r == role)
            .map(|(_, x, y)| (*x, *y))
            .unwrap_or((1, 1));
        let pos = json!({
            "x": suite_x + ox,
            "y": suite_y + oy,
            "room": suite_id,
        })
        .to_string();
        let r = sqlx::query(
            "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, manager_id, status) \
             VALUES (?, ?, ?, ?, ?, '', ?, ?, ?, 'idle')",
        )
        .bind(aid)
        .bind(&startup_id)
        .bind(role)
        .bind(role)
        .bind(backend)
        .bind(&pos)
        .bind(&suite_id)
        .bind(manager_id.map(|s| s.to_string()))
        .execute(&mut *tx)
        .await;
        if let Err(e) = r {
            return reply_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
    }

    if let Err(e) = tx.commit().await {
        return reply_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }

    // 4. mkdir workspaces/<id>/artifacts/. Best effort — log but don't 500
    //    since the row is already committed; M5.x cleanup can backfill.
    let artifacts = std::path::Path::new(&workspace_path).join("artifacts");
    if let Err(e) = tokio::fs::create_dir_all(&artifacts).await {
        tracing::warn!(component = "api_startups", error = %e, path = %artifacts.display(), "failed to create workspace artifacts dir");
    }

    // 5. Generate per-agent secrets, persist into process env, spawn workers.
    let world_url = std::env::var("CLIPTOWN_WORLD_WS_URL")
        .unwrap_or_else(|_| "ws://127.0.0.1:8080/ws/worker".to_string());

    // Test/dev override: when `CLIPTOWN_TEST_FIXED_AGENT_SECRET` is set, every
    // agent the API creates uses that value instead of a fresh UUID. Lets the
    // M9.10 real-LLM smoke (which spawns its own worker outside the supervisor)
    // know which secret to authenticate with. Unset in production → per-agent
    // random secrets, same as before.
    let fixed_test_secret = std::env::var("CLIPTOWN_TEST_FIXED_AGENT_SECRET").ok();
    // Test/dev override: when `CLIPTOWN_TEST_DISABLE_SUPERVISOR=1`, skip the
    // supervisor.spawn_agent call entirely. Used by the M9.10 real-LLM smoke,
    // which spawns its single worker out-of-band — letting the supervisor
    // try (and fail with the relative `packages/worker/bin/worker` path) just
    // pollutes world.log with `spawn_agent failed` warnings for 30s before
    // the backoff retries exhaust. Unset in production.
    let supervisor_disabled =
        std::env::var("CLIPTOWN_TEST_DISABLE_SUPERVISOR").as_deref() == Ok("1");
    for (_role, aid, backend, _mgr) in &agents {
        let secret = fixed_test_secret
            .clone()
            .unwrap_or_else(|| format!("ct-{}", Uuid::new_v4().simple()));
        // Worker children inherit this env var; auth.rs reads
        // `CLIPTOWN_AGENT_SECRET_<id>` to validate the hello.
        std::env::set_var(format!("CLIPTOWN_AGENT_SECRET_{}", aid), &secret);

        let cfg = SpawnConfig {
            agent_id: aid.to_string(),
            startup_id: startup_id.clone(),
            world_url: world_url.clone(),
            secret,
            workspace: workspace_path.clone(),
            backend: backend.to_string(),
            task: None,
        };
        if supervisor_disabled {
            continue;
        }
        // P3 Theme C follow-up: in per-task mode, the supervisor doesn't pre-
        // spawn long-running daemons here. The scheduler tick will call
        // `spawn_for_task` when a queued task is ready to dispatch, so daemons
        // at startup-creation time are redundant (and would conflict on WS
        // auth when the per-task worker connects with the same agent id).
        if per_task_workers_enabled() {
            continue;
        }
        if let Err(e) = s.supervisor.spawn_agent(cfg).await {
            tracing::warn!(component = "api_startups", agent_id = %aid, error = %e, "spawn_agent failed");
        }
    }

    // 6. Tell the world loop about the new avatars *and* the suite claim. The
    //    in-memory `avatars` map is what `mcp_dispatch` looks up (so worker
    //    MCP calls would otherwise fail with `unknown_agent`), and the
    //    in-memory `layout` is what `move_sys::can_enter_layout_room` reads
    //    (so without the claim the suite would still be public to other
    //    startups even though SQL says otherwise).
    let avatar_views: Vec<AvatarView> = agents
        .iter()
        .map(|(role, aid, backend, _)| {
            let (ox, oy) = HOME_DESK_OFFSETS
                .iter()
                .find(|(r, _, _)| r == role)
                .map(|(_, x, y)| (*x, *y))
                .unwrap_or((1, 1));
            AvatarView {
                agent_id: aid.to_string(),
                startup_id: startup_id.clone(),
                role: role.to_string(),
                backend: backend.to_string(),
                current_pos: (suite_x + ox, suite_y + oy),
                target_pos: None,
                room_id: suite_id.clone(),
                status: "idle".to_string(),
                last_seen_at: None,
                health: Health::Offline,
            }
        })
        .collect();
    let _ = s
        .handle
        .tx
        .send(Cmd::InsertAvatars {
            avatars: avatar_views,
            claim_suite: Some((suite_id.clone(), startup_id.clone())),
        })
        .await;

    let resp = CreateStartupResponse {
        id: startup_id,
        suite_id,
        agents: agents
            .iter()
            .map(|(role, aid, backend, _)| AgentRef {
                id: aid.to_string(),
                role: role.to_string(),
                backend: backend.to_string(),
            })
            .collect(),
    };
    Json(resp).into_response()
}

fn reply_err(code: StatusCode, message: &str) -> Response {
    (code, Json(json!({"error": message}))).into_response()
}

/// `DELETE /api/startups/:id` — dissolve a startup.
///
/// Phase 0 semantics:
/// - Mark the startup row `status = 'dissolved'` (row stays so foreign keys
///   from `tasks`/`budget_events`/`system_events`/`agents` keep referential
///   integrity, and so audit history remains queryable).
/// - Free the suite by clearing `rooms.private_to_startup_id` so the slot is
///   reusable by a subsequent POST.
/// - Fire-and-forget worker shutdown via M3.5's supervisor: SIGTERM, 5s grace,
///   SIGKILL. The DB transitions are committed *before* signalling so the
///   freed slot is observable immediately by the next create attempt — tests
///   don't have to wait for the kill grace window to elapse.
/// - `audit_trail` (JSON column on tasks), `budget_events`, and existing
///   `system_events` rows are intentionally untouched. We *append* a new
///   `startup_dissolved` system_event row at severity `warn`.
///
/// Idempotency note: calling this on a startup whose workers have already
/// exited (e.g. a re-issued DELETE, or a startup whose agents crashed past
/// their backoff budget) is safe — `dissolve_startup` snapshots the current
/// supervisor map and quietly does nothing for an empty match set.
pub async fn delete_startup(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    // Operator auth — same pattern as `http::patch_startup`. Validate BEFORE
    // the existence check so unauthenticated callers can't probe whether a
    // given startup id exists.
    let tok = extract_operator_token(&headers);
    if crate::auth::validate_operator_token(&s.pool, tok).await.is_err() {
        return reply_err(StatusCode::UNAUTHORIZED, "unauthorized");
    }

    // Verify startup exists. We do this outside the tx so a 404 doesn't
    // require a rollback — and the lookup matches what callers expect for
    // unknown ids.
    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM startups WHERE id = ?")
        .bind(&id)
        .fetch_optional(&s.pool)
        .await
        .ok()
        .flatten();
    if exists.is_none() {
        return reply_err(StatusCode::NOT_FOUND, "startup not found");
    }

    // Transaction: mark dissolved + free suite. Audit history (audit_trail
    // JSON on tasks, budget_events rows, system_events rows) is left intact.
    let mut tx = match s.pool.begin().await {
        Ok(t) => t,
        Err(e) => return reply_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if let Err(e) = sqlx::query("UPDATE startups SET status = 'dissolved' WHERE id = ?")
        .bind(&id)
        .execute(&mut *tx)
        .await
    {
        return reply_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    if let Err(e) =
        sqlx::query("UPDATE rooms SET private_to_startup_id = NULL WHERE private_to_startup_id = ?")
            .bind(&id)
            .execute(&mut *tx)
            .await
    {
        return reply_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    if let Err(e) = tx.commit().await {
        return reply_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }

    // Kill workers via M3.5 supervisor (SIGTERM → 5s grace → SIGKILL handled
    // inside). Idempotent: a no-op when no workers match this startup_id.
    s.supervisor.dissolve_startup(&id).await;

    // Mirror the SQL `private_to_startup_id = NULL` into the in-memory layout
    // so subsequent `move_sys::can_enter_layout_room` checks treat the freed
    // suite as public again. Without this, dissolved startups still own their
    // suite for the lifetime of the process.
    let _ = s
        .handle
        .tx
        .send(Cmd::ReleaseSuite { startup_id: id.clone() })
        .await;

    // Audit trail: emit a system_events row at `warn` and broadcast to the
    // operator console. Best-effort; a logging failure shouldn't fail the
    // user-facing DELETE since the dissolve is already committed.
    if let Err(e) = crate::emit::emit_system_event(
        &s.pool,
        &s.handle.event_tx,
        Some(&id),
        "startup_dissolved",
        &json!({"startup_id": id}).to_string(),
        "warn",
    )
    .await
    {
        tracing::error!(component = "api_startups", startup_id = %id, err = %e, "failed to emit startup_dissolved system_event");
    }

    Json(json!({"ok": true, "id": id, "status": "dissolved"})).into_response()
}
