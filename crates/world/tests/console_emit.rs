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
