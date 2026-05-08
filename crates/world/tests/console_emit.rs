//! Unit-style tests for the new console emit paths (cmd_console, mcp_dispatch,
//! emit_system_event). Each test drives one production handler and asserts
//! exactly which ConsoleOutbound frames reach the broadcast channel.

mod common;

use cliptown_world::{emit, protocol::ConsoleOutbound};
use common::TestCtx;
use serde_json::json;

#[tokio::test]
async fn emit_system_event_owns_id_and_ts() {
    let mut ctx = TestCtx::new().await;
    emit::emit_system_event(
        &ctx.pool,
        &ctx.event_tx,
        Some("s1"),
        "test_kind",
        &json!({"hello": "world"}).to_string(),
        "info",
    )
    .await
    .expect("emit_system_event should succeed");

    let frame = ctx.expect_one_broadcast();
    let ConsoleOutbound::SystemEvent {
        v, severity, kind, startup_id, payload, ts,
    } = frame else {
        panic!("expected SystemEvent, got {:?}", frame);
    };
    assert_eq!(v, 1);
    assert_eq!(severity, "info");
    assert_eq!(kind, "test_kind");
    assert_eq!(startup_id.as_deref(), Some("s1"));
    assert_eq!(payload, json!({"hello": "world"}));
    // ts is milliseconds — must be after epoch (>0) and before year 9999.
    assert!(ts > 1_000_000_000_000, "ts should be milliseconds, got {ts}");
    assert!(ts < 253_402_300_799_000, "ts should be milliseconds (< year 9999)");

    // SQL row exists with identical id/ts (seconds, not ms) and matching kind.
    let row: (String, i64, String) = sqlx::query_as(
        "SELECT id, ts, kind FROM system_events WHERE kind = 'test_kind'"
    )
    .fetch_one(&ctx.pool)
    .await
    .unwrap();
    assert_eq!(row.2, "test_kind");
    // SQL ts is seconds; broadcast ts was that times 1000.
    assert_eq!(row.1 * 1000, ts, "SQL ts (sec) should match broadcast ts (ms) / 1000");
}

async fn seed_agent(pool: &sqlx::SqlitePool, id: &str, startup_id: &str) {
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES (?, 'alpha', 'g', 10.0, 'town_default', '/tmp', 'active', unixepoch())",
    )
    .bind(startup_id).execute(pool).await.ok();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES (?, ?, 'F', 'founder', 'claude_code', 'm', '{}', 'suite_1', 'idle')",
    )
    .bind(id).bind(startup_id).execute(pool).await.unwrap();
}

#[tokio::test]
async fn broadcasts_on_operator_directive() {
    let mut ctx = TestCtx::new().await;
    seed_agent(&ctx.pool, "founder1", "s1").await;
    let mut w = cliptown_world::state::WorldView::default();

    let r = cliptown_world::cmd_console::dispatch(
        &mut w, &ctx.pool, &ctx.out_bus, &ctx.event_tx,
        serde_json::json!({
            "type": "operator_directive", "v": 1,
            "to_agent_id": "founder1",
            "body": "build the spec",
        }),
    ).await;
    assert_eq!(r["type"], "ok", "directive should succeed: {r}");
    let message_id = r["message_id"].as_str().unwrap().to_string();

    let frame = ctx.expect_one_broadcast();
    let cliptown_world::protocol::ConsoleOutbound::Directive {
        v, message_id: emitted_id, ts, startup_id, author_id, to_agent_id, body, in_response_to_task,
    } = frame else {
        panic!("expected Directive");
    };
    assert_eq!(v, 1);
    assert_eq!(emitted_id, message_id);
    assert!(ts > 1_000_000_000_000, "ts should be milliseconds");
    assert_eq!(startup_id, "s1");
    assert_eq!(author_id, "operator");
    assert_eq!(to_agent_id, "founder1");
    assert_eq!(body, "build the spec");
    assert_eq!(in_response_to_task, None);
}

#[tokio::test]
async fn no_broadcast_on_unknown_recipient() {
    let mut ctx = TestCtx::new().await;
    let mut w = cliptown_world::state::WorldView::default();

    let r = cliptown_world::cmd_console::dispatch(
        &mut w, &ctx.pool, &ctx.out_bus, &ctx.event_tx,
        serde_json::json!({
            "type": "operator_directive", "v": 1,
            "to_agent_id": "ghost",
            "body": "hi",
        }),
    ).await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "unknown_recipient");
    ctx.expect_no_broadcasts();
}
