mod common;

use cliptown_world::{cmd_console, seed, state::WorldView, storage};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::mpsc;

async fn fresh() -> (WorldView, sqlx::SqlitePool) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("test.db");
    let pool = storage::open(p.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    std::mem::forget(dir);
    (WorldView::default(), pool)
}

/// Empty out_bus for tests that don't care about worker outbound delivery.
fn empty_bus() -> HashMap<String, mpsc::Sender<Value>> {
    HashMap::new()
}

fn make_event_tx() -> (
    tokio::sync::broadcast::Sender<cliptown_world::protocol::ConsoleOutbound>,
    tokio::sync::broadcast::Receiver<cliptown_world::protocol::ConsoleOutbound>,
) {
    tokio::sync::broadcast::channel(64)
}

fn expect_no_broadcasts(
    rx: &mut tokio::sync::broadcast::Receiver<cliptown_world::protocol::ConsoleOutbound>,
) {
    let mut found = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(frame) => found.push(frame),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
        }
    }
    assert!(
        found.is_empty(),
        "expected no console broadcasts, found {} frame(s): {:?}",
        found.len(),
        found
    );
}

async fn insert_startup_agent_task(pool: &sqlx::SqlitePool, task_status: &str) {
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'alpha', 'goal', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
    ).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('a1', 's1', 'A1', 'engineer', 'claude_code', 'claude-3-5-sonnet', '{}', 'suite_1', 'idle')"
    ).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, created_at, updated_at) \
         VALUES ('T1', 's1', 'task one', 'desc', ?, unixepoch(), unixepoch())"
    ).bind(task_status).execute(pool).await.unwrap();
}

#[tokio::test]
async fn possess_inserts_operator_avatar() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, mut event_rx) = make_event_tx();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &cliptown_world::auth::OperatorIdentity::admin_for_tests(), json!({
        "type":"operator_possess","v":1,"startup_id":"s1"
    })).await;
    assert_eq!(r["type"], "ok");
    assert!(w.avatars.contains_key("__operator__"));
    expect_no_broadcasts(&mut event_rx);
}

#[tokio::test]
async fn unpossess_removes_operator_avatar() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, mut event_rx) = make_event_tx();
    cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &cliptown_world::auth::OperatorIdentity::admin_for_tests(), json!({
        "type":"operator_possess","v":1,"startup_id":"s1"
    })).await;
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &cliptown_world::auth::OperatorIdentity::admin_for_tests(), json!({
        "type":"operator_unpossess","v":1
    })).await;
    assert_eq!(r["type"], "ok");
    assert!(!w.avatars.contains_key("__operator__"));
    expect_no_broadcasts(&mut event_rx);
}

#[tokio::test]
async fn move_without_possess_errors() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, mut event_rx) = make_event_tx();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &cliptown_world::auth::OperatorIdentity::admin_for_tests(), json!({
        "type":"operator_move","v":1,"target_x":5,"target_y":3
    })).await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "not_possessing");
    expect_no_broadcasts(&mut event_rx);
}

#[tokio::test]
async fn directive_inserts_message_row() {
    let (mut w, pool) = fresh().await;
    insert_startup_agent_task(&pool, "queued").await;
    let bus = empty_bus();
    let (event_tx, mut event_rx) = make_event_tx();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &cliptown_world::auth::OperatorIdentity::admin_for_tests(), json!({
        "type":"operator_directive","v":1,"to_agent_id":"a1","body":"hi"
    })).await;
    assert_eq!(r["type"], "ok");
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM messages WHERE kind='directive'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(count.0, 1);
    // OperatorDirective now broadcasts a Directive frame after the SQL INSERT.
    match event_rx.try_recv() {
        Ok(cliptown_world::protocol::ConsoleOutbound::Directive {
            author_id, to_agent_id, body, in_response_to_task, ..
        }) => {
            assert_eq!(author_id, "operator");
            assert_eq!(to_agent_id, "a1");
            assert_eq!(body, "hi");
            assert_eq!(in_response_to_task, None);
        }
        other => panic!("expected Directive broadcast, got {:?}", other),
    }
    // No further broadcasts.
    expect_no_broadcasts(&mut event_rx);
}

#[tokio::test]
async fn accept_proposal_transitions_to_queued() {
    let (mut w, pool) = fresh().await;
    insert_startup_agent_task(&pool, "proposed").await;
    let bus = empty_bus();
    let (event_tx, mut event_rx) = make_event_tx();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &cliptown_world::auth::OperatorIdentity::admin_for_tests(), json!({
        "type":"operator_accept_proposal","v":1,"task_id":"T1","assignee_agent_id":"a1"
    })).await;
    assert_eq!(r["type"], "ok");
    let status: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(status.0, "queued");
    expect_no_broadcasts(&mut event_rx);
}

#[tokio::test]
async fn reject_proposal_transitions_to_failed() {
    let (mut w, pool) = fresh().await;
    insert_startup_agent_task(&pool, "proposed").await;
    let bus = empty_bus();
    let (event_tx, mut event_rx) = make_event_tx();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &cliptown_world::auth::OperatorIdentity::admin_for_tests(), json!({
        "type":"operator_reject_proposal","v":1,"task_id":"T1","reason":"bad scope"
    })).await;
    assert_eq!(r["type"], "ok");
    let status: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(status.0, "failed");
    expect_no_broadcasts(&mut event_rx);
}

#[tokio::test]
async fn force_accept_only_from_awaiting_review() {
    let (mut w, pool) = fresh().await;
    insert_startup_agent_task(&pool, "in_progress").await;
    let bus = empty_bus();
    let (event_tx, mut event_rx) = make_event_tx();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &cliptown_world::auth::OperatorIdentity::admin_for_tests(), json!({
        "type":"operator_force_accept","v":1,"task_id":"T1"
    })).await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "illegal_transition");
    expect_no_broadcasts(&mut event_rx);
}

#[tokio::test]
async fn force_accept_succeeds_from_awaiting_review() {
    let (mut w, pool) = fresh().await;
    insert_startup_agent_task(&pool, "awaiting_review").await;
    let bus = empty_bus();
    let (event_tx, mut event_rx) = make_event_tx();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &cliptown_world::auth::OperatorIdentity::admin_for_tests(), json!({
        "type":"operator_force_accept","v":1,"task_id":"T1"
    })).await;
    assert_eq!(r["type"], "ok");
    let status: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(status.0, "done");
    expect_no_broadcasts(&mut event_rx);
}

#[tokio::test]
async fn force_fail_with_note_writes_audit() {
    let (mut w, pool) = fresh().await;
    insert_startup_agent_task(&pool, "queued").await;
    let bus = empty_bus();
    let (event_tx, mut event_rx) = make_event_tx();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &cliptown_world::auth::OperatorIdentity::admin_for_tests(), json!({
        "type":"operator_force_fail","v":1,"task_id":"T1","note":"abandoned"
    })).await;
    assert_eq!(r["type"], "ok");
    let row: (String, String) = sqlx::query_as("SELECT status, audit_trail FROM tasks WHERE id='T1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(row.0, "failed");
    assert!(row.1.contains("force_fail"));
    assert!(row.1.contains("abandoned"));
    expect_no_broadcasts(&mut event_rx);
}

#[tokio::test]
async fn parse_error_returns_error_reply() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, mut event_rx) = make_event_tx();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &cliptown_world::auth::OperatorIdentity::admin_for_tests(), json!({
        "type":"unknown_op","v":1
    })).await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "parse");
    expect_no_broadcasts(&mut event_rx);
}

/// Codex round-3 P2#4: operator path mirrors the mcp_dispatch guard —
/// accepting a proposal with an assignee in a different startup is refused
/// before any state mutation, and the task stays in `proposed`.
#[tokio::test]
async fn accept_proposal_rejects_cross_startup_assignee() {
    let (mut w, pool) = fresh().await;
    // Two startups, each with one agent. The task lives in s1; we try to
    // assign s2's agent.
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'alpha', 'g', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s2', 'beta', 'g', 10.0, 'town_default', '/tmp/s2', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('a1', 's1', 'A1', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle')"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('a2', 's2', 'A2', 'engineer', 'claude_code', 'm', '{}', 'suite_3', 'idle')"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'proposed', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();

    let bus = empty_bus();
    let (event_tx, mut event_rx) = make_event_tx();
    let r = cmd_console::dispatch(
        &mut w,
        &pool,
        &bus,
        &event_tx, &cliptown_world::auth::OperatorIdentity::admin_for_tests(), json!({"type":"operator_accept_proposal","v":1,"task_id":"T1","assignee_agent_id":"a2"}),
    ).await;
    assert_eq!(r["type"], "error", "{r}");
    assert_eq!(r["reason"], "cross_startup", "{r}");
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT status, assignee_agent_id FROM tasks WHERE id='T1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "proposed");
    assert_eq!(row.1, None);
    expect_no_broadcasts(&mut event_rx);
}

/// P3 Theme B: viewer-role operators cannot trigger task-mutating arms; the
/// dispatcher returns `forbidden` before any SQL or broadcast.
#[tokio::test]
async fn viewer_role_cannot_force_accept_task() {
    let (mut w, pool) = fresh().await;
    insert_startup_agent_task(&pool, "awaiting_review").await;
    let bus = empty_bus();
    let (event_tx, mut event_rx) = make_event_tx();
    let viewer = cliptown_world::auth::OperatorIdentity {
        id: "op_v".into(),
        name: "viewer".into(),
        role: cliptown_world::auth::OperatorRole::Viewer,
    };
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &viewer, json!({
        "type":"operator_force_accept","v":1,"task_id":"T1"
    })).await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "forbidden");
    // SQL untouched.
    let row: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(row.0, "awaiting_review");
    expect_no_broadcasts(&mut event_rx);
}

/// P3 Theme B: viewer can still possess + move (read-ish operations).
#[tokio::test]
async fn viewer_role_can_possess_and_move() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _event_rx) = make_event_tx();
    let viewer = cliptown_world::auth::OperatorIdentity {
        id: "op_v".into(),
        name: "viewer".into(),
        role: cliptown_world::auth::OperatorRole::Viewer,
    };
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &viewer, json!({
        "type":"operator_possess","v":1,"startup_id":"s1"
    })).await;
    assert_eq!(r["type"], "ok");
    let r2 = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &viewer, json!({
        "type":"operator_move","v":1,"target_x":3,"target_y":4
    })).await;
    assert_eq!(r2["type"], "ok");
}

/// P3 Theme B: manager-role operators may force-accept tasks.
#[tokio::test]
async fn manager_role_can_force_accept_task() {
    let (mut w, pool) = fresh().await;
    insert_startup_agent_task(&pool, "awaiting_review").await;
    let bus = empty_bus();
    let (event_tx, _event_rx) = make_event_tx();
    let mgr = cliptown_world::auth::OperatorIdentity {
        id: "op_m".into(),
        name: "manager".into(),
        role: cliptown_world::auth::OperatorRole::Manager,
    };
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &mgr, json!({
        "type":"operator_force_accept","v":1,"task_id":"T1"
    })).await;
    assert_eq!(r["type"], "ok", "{r}");
    let row: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(row.0, "done");
}

// ── P3 Theme B follow-up: operator management commands (admin-only) ────────

fn admin() -> cliptown_world::auth::OperatorIdentity {
    cliptown_world::auth::OperatorIdentity::admin_for_tests()
}

fn viewer() -> cliptown_world::auth::OperatorIdentity {
    cliptown_world::auth::OperatorIdentity {
        id: "op_v".into(),
        name: "viewer".into(),
        role: cliptown_world::auth::OperatorRole::Viewer,
    }
}

#[tokio::test]
async fn operator_list_returns_seeded_admin_for_admin_caller() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _rx) = make_event_tx();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &admin(), json!({
        "type":"operator_list","v":1
    })).await;
    assert_eq!(r["type"], "ok");
    let ops = r["operators"].as_array().expect("operators array");
    // Migration 0003 seeds op_default. Any extra rows from prior tests in the
    // same in-memory db are fine; we just assert the seeded row is present.
    assert!(ops.iter().any(|o| o["id"] == "op_default" && o["role"] == "admin"));
}

#[tokio::test]
async fn operator_list_rejected_for_viewer() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _rx) = make_event_tx();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &viewer(), json!({
        "type":"operator_list","v":1
    })).await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "forbidden");
}

#[tokio::test]
async fn operator_create_mints_token_and_inserts_row() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _rx) = make_event_tx();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &admin(), json!({
        "type":"operator_create","v":1,"name":"alice","role":"manager"
    })).await;
    assert_eq!(r["type"], "ok");
    assert_eq!(r["name"], "alice");
    assert_eq!(r["role"], "manager");
    let token = r["token"].as_str().expect("token").to_string();
    assert!(token.starts_with("opt_"));
    // Round-trip: the minted token validates via the auth path.
    let id = cliptown_world::auth::validate_operator_token(&pool, &token).await.unwrap();
    assert_eq!(id.name, "alice");
    assert_eq!(id.role, cliptown_world::auth::OperatorRole::Manager);
}

#[tokio::test]
async fn operator_create_rejects_bad_role() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _rx) = make_event_tx();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &admin(), json!({
        "type":"operator_create","v":1,"name":"bob","role":"king"
    })).await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "bad_role");
}

#[tokio::test]
async fn operator_create_rejects_duplicate_name() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _rx) = make_event_tx();
    // Migration 0003 already seeds 'default-admin'.
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &admin(), json!({
        "type":"operator_create","v":1,"name":"default-admin","role":"viewer"
    })).await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "name_taken");
}

#[tokio::test]
async fn operator_revoke_removes_row() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _rx) = make_event_tx();
    sqlx::query("INSERT INTO operators (id, name, token, role, created_at) VALUES ('op_x','x','tok_x','viewer',unixepoch())")
        .execute(&pool).await.unwrap();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &admin(), json!({
        "type":"operator_revoke","v":1,"operator_id":"op_x"
    })).await;
    assert_eq!(r["type"], "ok");
    // Token no longer validates.
    assert!(cliptown_world::auth::validate_operator_token(&pool, "tok_x").await.is_err());
}

#[tokio::test]
async fn operator_revoke_refuses_self() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _rx) = make_event_tx();
    // The admin_for_tests identity has id = "op_test" — insert that row so
    // the self-revoke path is meaningful.
    sqlx::query("INSERT INTO operators (id, name, token, role, created_at) VALUES ('op_test','self','tok_self','admin',unixepoch())")
        .execute(&pool).await.unwrap();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &admin(), json!({
        "type":"operator_revoke","v":1,"operator_id":"op_test"
    })).await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "cannot_revoke_self");
}

#[tokio::test]
async fn operator_set_role_changes_row() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _rx) = make_event_tx();
    sqlx::query("INSERT INTO operators (id, name, token, role, created_at) VALUES ('op_y','y','tok_y','viewer',unixepoch())")
        .execute(&pool).await.unwrap();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &admin(), json!({
        "type":"operator_set_role","v":1,"operator_id":"op_y","role":"manager"
    })).await;
    assert_eq!(r["type"], "ok");
    let id = cliptown_world::auth::validate_operator_token(&pool, "tok_y").await.unwrap();
    assert_eq!(id.role, cliptown_world::auth::OperatorRole::Manager);
}

// ── P3 Theme F follow-up: operator-side skill authoring ───────────────────

#[tokio::test]
async fn skill_upsert_operator_creates_then_updates() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _rx) = make_event_tx();
    // Seed startup so cross-startup check passes.
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'a', 'g', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &admin(), json!({
        "type":"skill_upsert_operator","v":1,"startup_id":"s1","skill_id":null,
        "name":"my-skill","content_md":"first content"
    })).await;
    assert_eq!(r["type"], "ok");
    let id = r["skill_id"].as_str().unwrap().to_string();
    // Re-submit with same name — should update in place, not duplicate.
    let r2 = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &admin(), json!({
        "type":"skill_upsert_operator","v":1,"startup_id":"s1","skill_id":null,
        "name":"my-skill","content_md":"second content"
    })).await;
    assert_eq!(r2["type"], "ok");
    assert_eq!(r2["skill_id"].as_str().unwrap(), id);
    let row: (String, String) = sqlx::query_as("SELECT id, content_md FROM skills WHERE name='my-skill' AND startup_id='s1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(row.0, id);
    assert_eq!(row.1, "second content");
}

#[tokio::test]
async fn skill_upsert_operator_viewer_forbidden() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _rx) = make_event_tx();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'a', 'g', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &viewer(), json!({
        "type":"skill_upsert_operator","v":1,"startup_id":"s1","skill_id":null,
        "name":"my-skill","content_md":"x"
    })).await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "forbidden");
}

#[tokio::test]
async fn skill_delete_operator_removes_row() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _rx) = make_event_tx();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'a', 'g', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO skills (id, startup_id, name, content_md, created_at, updated_at) \
         VALUES ('sk_a','s1','a','c',unixepoch(),unixepoch())"
    ).execute(&pool).await.unwrap();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &admin(), json!({
        "type":"skill_delete_operator","v":1,"startup_id":"s1","skill_id":"sk_a"
    })).await;
    assert_eq!(r["type"], "ok");
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM skills WHERE id='sk_a'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn skill_delete_operator_viewer_forbidden() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _rx) = make_event_tx();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'a', 'g', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO skills (id, startup_id, name, content_md, created_at, updated_at) \
         VALUES ('sk_b','s1','b','c',unixepoch(),unixepoch())"
    ).execute(&pool).await.unwrap();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &viewer(), json!({
        "type":"skill_delete_operator","v":1,"startup_id":"s1","skill_id":"sk_b"
    })).await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "forbidden");
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM skills WHERE id='sk_b'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(count.0, 1, "viewer attempt must not touch SQL");
}

#[tokio::test]
async fn operator_set_role_refuses_self_demotion() {
    let (mut w, pool) = fresh().await;
    let bus = empty_bus();
    let (event_tx, _rx) = make_event_tx();
    sqlx::query("INSERT INTO operators (id, name, token, role, created_at) VALUES ('op_test','self','tok_self','admin',unixepoch())")
        .execute(&pool).await.unwrap();
    let r = cmd_console::dispatch(&mut w, &pool, &bus, &event_tx, &admin(), json!({
        "type":"operator_set_role","v":1,"operator_id":"op_test","role":"viewer"
    })).await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "cannot_demote_self");
}
