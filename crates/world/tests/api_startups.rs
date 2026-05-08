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
        .header("Authorization", "Bearer dev-token")
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
        .header("Authorization", "Bearer dev-token")
        .body(Body::from(body.to_string())).unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn fifth_post_returns_409_after_4_succeed() {
    let state = fixture().await;

    let mut workspaces = Vec::new();
    for i in 0..4 {
        let app = router(state.clone());
        let body = serde_json::json!({
            "name": format!("startup-{}", i),
            "goal_text": "x",
            "budget_cap_usd": 5.0,
            "backends": { "founder": "claude_code", "engineer": "claude_code", "designer": "claude_code" }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/startups")
            .header("Content-Type", "application/json")
            .header("Authorization", "Bearer dev-token")
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "create #{} should succeed", i);

        let body = to_bytes(res.into_body(), 4096).await.unwrap();
        let resp_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = resp_json.get("id").and_then(|v| v.as_str()).unwrap().to_string();
        let workspace: (String,) = sqlx::query_as("SELECT workspace_path FROM startups WHERE id = ?")
            .bind(&id).fetch_one(&state.pool).await.unwrap();
        workspaces.push(workspace.0);
    }

    // 5th create — no free suites left.
    let app = router(state.clone());
    let body = serde_json::json!({
        "name": "fifth",
        "goal_text": "x",
        "budget_cap_usd": 5.0,
        "backends": { "founder": "claude_code", "engineer": "claude_code", "designer": "claude_code" }
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/startups")
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer dev-token")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);

    // Verify exactly 4 startups + 12 agents in DB.
    let s_count: (i64,) = sqlx::query_as("SELECT count(*) FROM startups")
        .fetch_one(&state.pool).await.unwrap();
    assert_eq!(s_count.0, 4);
    let a_count: (i64,) = sqlx::query_as("SELECT count(*) FROM agents")
        .fetch_one(&state.pool).await.unwrap();
    assert_eq!(a_count.0, 12);

    // All 4 suites are claimed.
    let claimed: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM rooms WHERE type = 'office' AND private_to_startup_id IS NOT NULL"
    ).fetch_one(&state.pool).await.unwrap();
    assert_eq!(claimed.0, 4);

    // Cleanup workspace dirs from the 4 successful creates.
    for ws in workspaces {
        let _ = std::fs::remove_dir_all(&ws);
    }
}

#[tokio::test]
async fn rejected_create_leaves_no_orphan_rows() {
    let state = fixture().await;
    // Pre-claim all 4 suites.
    sqlx::query("UPDATE rooms SET private_to_startup_id = 'placeholder' WHERE type = 'office'")
        .execute(&state.pool).await.unwrap();
    let app = router(state.clone());

    let body = serde_json::json!({
        "name": "doomed",
        "goal_text": "x",
        "budget_cap_usd": 5.0,
        "backends": { "founder": "claude_code", "engineer": "claude_code", "designer": "claude_code" }
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/startups")
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer dev-token")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);

    // No partial writes — startups + agents both empty.
    let s_count: (i64,) = sqlx::query_as("SELECT count(*) FROM startups")
        .fetch_one(&state.pool).await.unwrap();
    assert_eq!(s_count.0, 0);
    let a_count: (i64,) = sqlx::query_as("SELECT count(*) FROM agents")
        .fetch_one(&state.pool).await.unwrap();
    assert_eq!(a_count.0, 0);
}

#[tokio::test]
async fn delete_marks_dissolved_and_frees_suite() {
    let state = fixture().await;
    let app = router(state.clone());

    // Create one startup.
    let body = serde_json::json!({
        "name": "alpha", "goal_text": "x", "budget_cap_usd": 5.0,
        "backends": { "founder": "claude_code", "engineer": "claude_code", "designer": "claude_code" }
    });
    let req = Request::builder().method("POST").uri("/api/startups")
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer dev-token")
        .body(Body::from(body.to_string())).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let resp_bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let resp_json: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
    let startup_id = resp_json["id"].as_str().unwrap().to_string();
    let workspace: (String,) = sqlx::query_as("SELECT workspace_path FROM startups WHERE id = ?")
        .bind(&startup_id).fetch_one(&state.pool).await.unwrap();

    // Verify suite claimed.
    let claimed: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM rooms WHERE private_to_startup_id = ?"
    ).bind(&startup_id).fetch_one(&state.pool).await.unwrap();
    assert_eq!(claimed.0, 1);

    // DELETE. Operator token uses the default dev token (`auth.rs`).
    let req = Request::builder()
        .method("DELETE").uri(format!("/api/startups/{}", startup_id))
        .header("Authorization", "Bearer dev-token")
        .body(Body::empty()).unwrap();
    let res = router(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Status: dissolved.
    let row: (String,) = sqlx::query_as("SELECT status FROM startups WHERE id = ?")
        .bind(&startup_id).fetch_one(&state.pool).await.unwrap();
    assert_eq!(row.0, "dissolved");

    // Suite freed.
    let claimed: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM rooms WHERE private_to_startup_id = ?"
    ).bind(&startup_id).fetch_one(&state.pool).await.unwrap();
    assert_eq!(claimed.0, 0);

    // system_events alert.
    let count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM system_events WHERE kind = 'startup_dissolved'"
    ).fetch_one(&state.pool).await.unwrap();
    assert_eq!(count.0, 1);

    // Cleanup workspace dir.
    let _ = std::fs::remove_dir_all(&workspace.0);
}

#[tokio::test]
async fn delete_then_create_succeeds_after_exhaustion() {
    let state = fixture().await;

    // Fill all 4 suites.
    let mut ids = Vec::new();
    let mut workspaces = Vec::new();
    for i in 0..4 {
        let body = serde_json::json!({
            "name": format!("s-{}", i), "goal_text": "x", "budget_cap_usd": 5.0,
            "backends": { "founder": "claude_code", "engineer": "claude_code", "designer": "claude_code" }
        });
        let req = Request::builder().method("POST").uri("/api/startups")
            .header("Content-Type", "application/json")
            .header("Authorization", "Bearer dev-token")
            .body(Body::from(body.to_string())).unwrap();
        let res = router(state.clone()).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bs = to_bytes(res.into_body(), 8192).await.unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bs).unwrap();
        let id = j["id"].as_str().unwrap().to_string();
        let ws: (String,) = sqlx::query_as("SELECT workspace_path FROM startups WHERE id = ?")
            .bind(&id).fetch_one(&state.pool).await.unwrap();
        workspaces.push(ws.0);
        ids.push(id);
    }

    // 5th: 409.
    let body = serde_json::json!({
        "name": "fifth", "goal_text": "x", "budget_cap_usd": 5.0,
        "backends": { "founder": "claude_code", "engineer": "claude_code", "designer": "claude_code" }
    });
    let req = Request::builder().method("POST").uri("/api/startups")
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer dev-token")
        .body(Body::from(body.to_string())).unwrap();
    let res = router(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);

    // DELETE the first.
    let req = Request::builder().method("DELETE").uri(format!("/api/startups/{}", ids[0]))
        .header("Authorization", "Bearer dev-token")
        .body(Body::empty()).unwrap();
    let res = router(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 5th retry: now succeeds.
    let body = serde_json::json!({
        "name": "fifth", "goal_text": "x", "budget_cap_usd": 5.0,
        "backends": { "founder": "claude_code", "engineer": "claude_code", "designer": "claude_code" }
    });
    let req = Request::builder().method("POST").uri("/api/startups")
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer dev-token")
        .body(Body::from(body.to_string())).unwrap();
    let res = router(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bs = to_bytes(res.into_body(), 8192).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bs).unwrap();
    let fifth_id = j["id"].as_str().unwrap().to_string();
    let ws: (String,) = sqlx::query_as("SELECT workspace_path FROM startups WHERE id = ?")
        .bind(&fifth_id).fetch_one(&state.pool).await.unwrap();
    workspaces.push(ws.0);

    // Cleanup workspace dirs.
    for ws in workspaces {
        let _ = std::fs::remove_dir_all(&ws);
    }
}

#[tokio::test]
async fn delete_unknown_returns_404() {
    let state = fixture().await;
    let app = router(state.clone());
    let req = Request::builder().method("DELETE").uri("/api/startups/does-not-exist")
        .header("Authorization", "Bearer dev-token")
        .body(Body::empty()).unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// Without an operator token, DELETE must reject with 401 BEFORE doing the
/// existence check — otherwise an unauthenticated probe could distinguish
/// known from unknown ids.
#[tokio::test]
async fn delete_without_auth_returns_401() {
    let state = fixture().await;
    let app = router(state.clone());
    // Use a real id that exists, so a missing-auth bypass would 200 (not 404).
    let body = serde_json::json!({
        "name": "alpha", "goal_text": "x", "budget_cap_usd": 5.0,
        "backends": { "founder": "claude_code", "engineer": "claude_code", "designer": "claude_code" }
    });
    let req = Request::builder().method("POST").uri("/api/startups")
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer dev-token")
        .body(Body::from(body.to_string())).unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bs = to_bytes(res.into_body(), 8192).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bs).unwrap();
    let id = j["id"].as_str().unwrap().to_string();
    let workspace: (String,) = sqlx::query_as("SELECT workspace_path FROM startups WHERE id = ?")
        .bind(&id).fetch_one(&state.pool).await.unwrap();

    // No Authorization header.
    let req = Request::builder().method("DELETE").uri(format!("/api/startups/{}", id))
        .body(Body::empty()).unwrap();
    let res = router(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Wrong token.
    let req = Request::builder().method("DELETE").uri(format!("/api/startups/{}", id))
        .header("Authorization", "Bearer WRONG")
        .body(Body::empty()).unwrap();
    let res = router(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Status remains active — auth failure must not have side effects.
    let row: (String,) = sqlx::query_as("SELECT status FROM startups WHERE id = ?")
        .bind(&id).fetch_one(&state.pool).await.unwrap();
    assert_eq!(row.0, "active");

    // Cleanup workspace dir.
    let _ = std::fs::remove_dir_all(&workspace.0);
}

/// `POST /api/startups` must seed the in-memory `WorldView.avatars` map with
/// the 3 new agents (so worker MCP calls + scheduler dispatch can find them)
/// and mark the claimed suite as private in the loop's layout (so other
/// startups cannot pathfind into it).
#[tokio::test]
async fn post_inserts_avatars_and_claims_suite_in_world_view() {
    let state = fixture().await;
    let app = router(state.clone());
    let body = serde_json::json!({
        "name": "alpha", "goal_text": "x", "budget_cap_usd": 5.0,
        "backends": { "founder": "claude_code", "engineer": "claude_code", "designer": "claude_code" }
    });
    let req = Request::builder().method("POST").uri("/api/startups")
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer dev-token")
        .body(Body::from(body.to_string())).unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bs = to_bytes(res.into_body(), 8192).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bs).unwrap();
    let startup_id = j["id"].as_str().unwrap().to_string();
    let suite_id = j["suite_id"].as_str().unwrap().to_string();
    let workspace: (String,) = sqlx::query_as("SELECT workspace_path FROM startups WHERE id = ?")
        .bind(&startup_id).fetch_one(&state.pool).await.unwrap();

    // Wait for the InsertAvatars cmd to land. The handler send is fire-and-
    // forget on an mpsc channel that the loop drains on its next select tick;
    // give it a beat. We poll for up to ~500ms.
    let mut found = 0usize;
    for _ in 0..50 {
        let view = state.handle.view_rx.borrow().clone();
        found = view
            .avatars
            .values()
            .filter(|a| a.startup_id == startup_id)
            .count();
        if found == 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(found, 3, "expected 3 avatars in WorldView; got {}", found);

    // Each avatar should be in the claimed suite at one of the home-desk
    // offsets we hand-wrote in api_startups::HOME_DESK_OFFSETS.
    let view = state.handle.view_rx.borrow().clone();
    for a in view.avatars.values().filter(|a| a.startup_id == startup_id) {
        assert_eq!(a.room_id, suite_id, "avatar {} not in suite", a.agent_id);
        assert_eq!(a.status, "idle");
    }

    // Suite-claim sync: ask the world loop to start a move from a *different*
    // startup's agent into this suite — it must fail with permission_denied.
    // We stage that by inserting a synthetic avatar belonging to a fake other
    // startup, then sending a move_intent through the same path the worker
    // does (via Cmd::HandleWorkerMsg).
    let other_avatar = cliptown_world::state::AvatarView {
        agent_id: "intruder".to_string(),
        startup_id: "other-startup".to_string(),
        role: "founder".to_string(),
        backend: "claude_code".to_string(),
        current_pos: (20, 5), // lobby
        target_pos: None,
        room_id: "lobby".to_string(),
        status: "idle".to_string(),
    };
    let _ = state
        .handle
        .tx
        .send(cliptown_world::loop_::Cmd::InsertAvatars {
            avatars: vec![other_avatar],
            claim_suite: None,
        })
        .await;

    // Insert a row so auth wouldn't bounce a hypothetical worker connect (we
    // don't need it for the loop dispatch path, but it keeps the test honest).
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('other-startup', 'beta', 'g', 1.0, 'town_default', '/tmp/beta', 'active', unixepoch())"
    ).execute(&state.pool).await.unwrap();

    // Ask the loop to start a move into the claimed suite. We use
    // HandleWorkerMsg so the actual move_sys::start_move runs against the
    // loop's owned (mutable) layout.
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let intent = serde_json::json!({
        "type": "move_intent",
        "v": 1,
        "target_room": suite_id,
        "target_x": 1,
        "target_y": 1,
    });
    state
        .handle
        .tx
        .send(cliptown_world::loop_::Cmd::HandleWorkerMsg {
            agent_id: "intruder".to_string(),
            msg: intent,
            reply: reply_tx,
        })
        .await
        .unwrap();
    let reply = reply_rx.await.unwrap();
    assert_eq!(
        reply.get("reason").and_then(|v| v.as_str()),
        Some("no_permission"),
        "intruder should not be able to move into the claimed suite; got {reply}"
    );

    // Cleanup.
    let _ = std::fs::remove_dir_all(&workspace.0);
}

/// After DELETE, the suite ownership is cleared in the in-memory layout so
/// another startup can pathfind in. Mirrors the SQL `private_to_startup_id =
/// NULL` write — without `Cmd::ReleaseSuite` the in-memory map would diverge
/// for the lifetime of the process.
#[tokio::test]
async fn delete_releases_suite_in_world_view() {
    let state = fixture().await;
    let app = router(state.clone());

    // Create + delete a startup.
    let body = serde_json::json!({
        "name": "alpha", "goal_text": "x", "budget_cap_usd": 5.0,
        "backends": { "founder": "claude_code", "engineer": "claude_code", "designer": "claude_code" }
    });
    let req = Request::builder().method("POST").uri("/api/startups")
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer dev-token")
        .body(Body::from(body.to_string())).unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bs = to_bytes(res.into_body(), 8192).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bs).unwrap();
    let startup_id = j["id"].as_str().unwrap().to_string();
    let suite_id = j["suite_id"].as_str().unwrap().to_string();
    let workspace: (String,) = sqlx::query_as("SELECT workspace_path FROM startups WHERE id = ?")
        .bind(&startup_id).fetch_one(&state.pool).await.unwrap();

    // Wait for InsertAvatars to land before issuing DELETE (so the claim is
    // observable by the cleanup path).
    for _ in 0..50 {
        let view = state.handle.view_rx.borrow().clone();
        if view.avatars.values().filter(|a| a.startup_id == startup_id).count() == 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let req = Request::builder().method("DELETE").uri(format!("/api/startups/{}", startup_id))
        .header("Authorization", "Bearer dev-token")
        .body(Body::empty()).unwrap();
    let res = router(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Dissolved startup's avatars must be removed from the world view.
    // Without this, console snapshots, proximity, and the scheduler keep
    // seeing ghost agents until process restart. Poll briefly so the
    // ReleaseSuite cmd has time to drain.
    let mut still_present = usize::MAX;
    for _ in 0..50 {
        let view = state.handle.view_rx.borrow().clone();
        still_present = view
            .avatars
            .values()
            .filter(|a| a.startup_id == startup_id)
            .count();
        if still_present == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        still_present, 0,
        "expected 0 avatars for dissolved startup; got {}",
        still_present
    );

    // Insert a foreign avatar in the lobby — it must now be allowed into the
    // freed suite.
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('other-startup', 'beta', 'g', 1.0, 'town_default', '/tmp/beta', 'active', unixepoch())"
    ).execute(&state.pool).await.unwrap();
    let other = cliptown_world::state::AvatarView {
        agent_id: "free_agent".to_string(),
        startup_id: "other-startup".to_string(),
        role: "founder".to_string(),
        backend: "claude_code".to_string(),
        current_pos: (20, 5),
        target_pos: None,
        room_id: "lobby".to_string(),
        status: "idle".to_string(),
    };
    let _ = state
        .handle
        .tx
        .send(cliptown_world::loop_::Cmd::InsertAvatars {
            avatars: vec![other],
            claim_suite: None,
        })
        .await;

    // Wait for ReleaseSuite + InsertAvatars to drain.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let intent = serde_json::json!({
        "type": "move_intent",
        "v": 1,
        "target_room": suite_id,
        "target_x": 1,
        "target_y": 1,
    });
    state
        .handle
        .tx
        .send(cliptown_world::loop_::Cmd::HandleWorkerMsg {
            agent_id: "free_agent".to_string(),
            msg: intent,
            reply: reply_tx,
        })
        .await
        .unwrap();
    let reply = reply_rx.await.unwrap();
    assert_eq!(
        reply.get("type").and_then(|v| v.as_str()),
        Some("ok"),
        "free_agent should be able to enter the freed suite; got {reply}"
    );

    let _ = std::fs::remove_dir_all(&workspace.0);
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
        .header("Authorization", "Bearer dev-token")
        .body(Body::from(body.to_string())).unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// `POST /api/startups` must require an operator token. Without one (or with
/// the wrong token), the call is rejected at 401 BEFORE any DB writes — so an
/// unauthenticated probe can't claim a suite slot, leak a startup id, or
/// burn a backend allocation.
#[tokio::test]
async fn post_without_auth_returns_401_and_writes_nothing() {
    let state = fixture().await;
    let body = serde_json::json!({
        "name": "alpha", "goal_text": "x", "budget_cap_usd": 5.0,
        "backends": { "founder": "claude_code", "engineer": "claude_code", "designer": "claude_code" }
    });

    // No Authorization header at all.
    let req = Request::builder()
        .method("POST").uri("/api/startups")
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string())).unwrap();
    let res = router(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Wrong token.
    let req = Request::builder()
        .method("POST").uri("/api/startups")
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer WRONG")
        .body(Body::from(body.to_string())).unwrap();
    let res = router(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // No partial DB writes — startups + agents tables remain empty, and no
    // suite has been claimed (so the next authenticated POST can pick suite_1).
    let s_count: (i64,) = sqlx::query_as("SELECT count(*) FROM startups")
        .fetch_one(&state.pool).await.unwrap();
    assert_eq!(s_count.0, 0);
    let a_count: (i64,) = sqlx::query_as("SELECT count(*) FROM agents")
        .fetch_one(&state.pool).await.unwrap();
    assert_eq!(a_count.0, 0);
    let claimed: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM rooms WHERE type = 'office' AND private_to_startup_id IS NOT NULL"
    ).fetch_one(&state.pool).await.unwrap();
    assert_eq!(claimed.0, 0);
}
