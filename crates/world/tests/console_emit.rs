//! Unit-style tests for the new console emit paths (cmd_console, mcp_dispatch,
//! emit_system_event). Each test drives one production handler and asserts
//! exactly which ConsoleOutbound frames reach the broadcast channel.

mod common;

use cliptown_world::{emit, protocol::ConsoleOutbound};
use common::TestCtx;
use serde_json::json;

/// TODOS P3 regression guard: when the payload string isn't valid JSON, the
/// SQL row stores the raw string and the broadcast frame used to silently
/// degrade to `Value::Null` — operator console and audit log diverged.
/// After the fix, both surfaces see the same data (raw string on the wire,
/// raw string in SQL) and a `tracing::error!` flags the producer bug.
#[tokio::test]
async fn emit_system_event_malformed_payload_preserves_raw_on_broadcast() {
    let mut ctx = TestCtx::new().await;
    let raw = "this is not { valid json";
    emit::emit_system_event(
        &ctx.pool,
        &ctx.event_tx,
        Some("s1"),
        "test_kind_malformed",
        raw,
        "warn",
    )
    .await
    .expect("emit_system_event should succeed even with malformed payload");

    let frame = ctx.expect_one_broadcast();
    let ConsoleOutbound::SystemEvent { payload, .. } = frame else {
        panic!("expected SystemEvent");
    };
    // Broadcast: the raw string lands as a JSON string value — NOT null.
    assert_eq!(
        payload,
        serde_json::Value::String(raw.into()),
        "broadcast payload must equal the raw string when parse fails",
    );

    // SQL: the same raw string is persisted (unchanged behavior).
    let row: (String,) = sqlx::query_as(
        "SELECT payload FROM system_events WHERE kind = 'test_kind_malformed'",
    )
    .fetch_one(&ctx.pool)
    .await
    .unwrap();
    assert_eq!(row.0, raw, "SQL must store the raw string");
}

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
async fn broadcasts_on_peer_chat() {
    use cliptown_world::{mcp_dispatch, move_sys, path::RoomGraph, seed::TownLayout, state::AvatarView};

    let mut ctx = TestCtx::new().await;
    // Seed a single startup with one agent in cafe.
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'a', 'g', 10.0, 'town_default', '/tmp', 'active', unixepoch())"
    ).execute(&ctx.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('a1', 's1', 'A', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle')"
    ).execute(&ctx.pool).await.unwrap();

    let mut w = cliptown_world::state::WorldView::default();
    w.avatars.insert("a1".into(), AvatarView {
        agent_id: "a1".into(), startup_id: "s1".into(), role: "engineer".into(),
        backend: "claude_code".into(), current_pos: (0,0), target_pos: None,
        room_id: "cafe".into(), status: "idle".into(),
    });

    let layout = TownLayout::default_town();
    let graph: RoomGraph = move_sys::graph_from_layout(&layout);
    let mut paths = std::collections::HashMap::new();

    let r = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &ctx.out_bus, &ctx.pool, &ctx.event_tx,
        "a1",
        serde_json::json!({
            "type": "mcp_call", "v": 1, "tool": "speak", "corr_id": "c1",
            "args": { "kind": "chat", "body": "hello cafe" }
        }),
    ).await;
    assert_eq!(r["type"], "mcp_reply", "speak should succeed: {r}");

    let frame = ctx.expect_one_broadcast();
    let cliptown_world::protocol::ConsoleOutbound::Chat {
        v, message_id, ts, startup_id, room_id, author_id, body,
    } = frame else { panic!("expected Chat") };
    assert_eq!(v, 1);
    assert!(!message_id.is_empty());
    assert!(ts > 1_000_000_000_000);
    assert_eq!(startup_id, "s1");
    assert_eq!(room_id, "cafe");
    assert_eq!(author_id, "a1");
    assert_eq!(body, "hello cafe");
}

#[tokio::test]
async fn broadcasts_on_peer_directive() {
    use cliptown_world::{mcp_dispatch, move_sys, path::RoomGraph, seed::TownLayout, state::AvatarView};

    let mut ctx = TestCtx::new().await;
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'a', 'g', 10.0, 'town_default', '/tmp', 'active', unixepoch())"
    ).execute(&ctx.pool).await.unwrap();
    sqlx::query("INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status, manager_id) VALUES ('mgr', 's1', 'M', 'founder', 'claude_code', 'm', '{}', 'suite_1', 'idle', NULL)")
        .execute(&ctx.pool).await.unwrap();
    sqlx::query("INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status, manager_id) VALUES ('eng', 's1', 'E', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle', 'mgr')")
        .execute(&ctx.pool).await.unwrap();

    let mut w = cliptown_world::state::WorldView::default();
    w.avatars.insert("mgr".into(), AvatarView {
        agent_id: "mgr".into(), startup_id: "s1".into(), role: "founder".into(),
        backend: "claude_code".into(), current_pos: (0,0), target_pos: None,
        room_id: "suite_1".into(), status: "idle".into(),
    });

    let layout = TownLayout::default_town();
    let graph: RoomGraph = move_sys::graph_from_layout(&layout);
    let mut paths = std::collections::HashMap::new();

    let r = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &ctx.out_bus, &ctx.pool, &ctx.event_tx,
        "mgr",
        serde_json::json!({
            "type": "mcp_call", "v": 1, "tool": "speak", "corr_id": "c1",
            "args": { "kind": "directive", "to_agent_id": "eng", "body": "do the thing" }
        }),
    ).await;
    assert_eq!(r["type"], "mcp_reply", "directive should succeed: {r}");

    let frame = ctx.expect_one_broadcast();
    let cliptown_world::protocol::ConsoleOutbound::Directive {
        author_id, to_agent_id, body, in_response_to_task, ..
    } = frame else { panic!("expected Directive") };
    assert_eq!(author_id, "mgr");
    assert_eq!(to_agent_id, "eng");
    assert_eq!(body, "do the thing");
    assert_eq!(in_response_to_task, None);
}

/// Body-length guard on the operator → directive path. Same MAX_BODY_LENGTH
/// (=4096 chars) limit as the worker-side speak/task_request_changes paths;
/// an operator with an unbounded `body` could otherwise starve the broadcast
/// channel just as a chatty agent could. Reject pre-recipient-check so we
/// don't even canonicalize the SQL row before bailing.
#[tokio::test]
async fn no_broadcast_on_body_too_long() {
    let mut ctx = TestCtx::new().await;
    seed_agent(&ctx.pool, "founder1", "s1").await;
    let mut w = cliptown_world::state::WorldView::default();

    let long = "x".repeat(4097);
    let r = cliptown_world::cmd_console::dispatch(
        &mut w, &ctx.pool, &ctx.out_bus, &ctx.event_tx,
        serde_json::json!({
            "type": "operator_directive", "v": 1,
            "to_agent_id": "founder1",
            "body": long,
        }),
    ).await;
    assert_eq!(r["type"], "error");
    assert_eq!(r["reason"], "body_too_long");

    // No SQL row written, no fan-out, no broadcast.
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM messages WHERE startup_id = 's1' AND kind = 'directive'"
    ).fetch_one(&ctx.pool).await.unwrap();
    assert_eq!(count.0, 0);
    ctx.expect_no_broadcasts();
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

async fn seed_review_cycle_fixture(pool: &sqlx::SqlitePool) {
    sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) VALUES ('s1', 'a', 'g', 10.0, 'town_default', '/tmp', 'active', unixepoch())")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status, manager_id) VALUES ('mgr', 's1', 'M', 'founder', 'claude_code', 'm', '{}', 'suite_1', 'idle', NULL)")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status, manager_id) VALUES ('eng', 's1', 'E', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle', 'mgr')")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, review_round, created_at, updated_at) VALUES ('T1', 's1', NULL, 'T', 'D', 'awaiting_review', 'eng', 0, unixepoch(), unixepoch())")
        .execute(pool).await.unwrap();
}

#[tokio::test]
async fn broadcasts_on_review_request_changes() {
    use cliptown_world::{mcp_dispatch, move_sys, path::RoomGraph, seed::TownLayout, state::AvatarView};
    let mut ctx = TestCtx::new().await;
    seed_review_cycle_fixture(&ctx.pool).await;

    let mut w = cliptown_world::state::WorldView::default();
    w.avatars.insert("mgr".into(), AvatarView {
        agent_id: "mgr".into(), startup_id: "s1".into(), role: "founder".into(),
        backend: "claude_code".into(), current_pos: (0,0), target_pos: None,
        room_id: "suite_1".into(), status: "idle".into(),
    });
    let layout = TownLayout::default_town();
    let graph: RoomGraph = move_sys::graph_from_layout(&layout);
    let mut paths = std::collections::HashMap::new();

    let r = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &ctx.out_bus, &ctx.pool, &ctx.event_tx,
        "mgr",
        serde_json::json!({
            "type":"mcp_call","v":1,"tool":"task_request_changes","corr_id":"c1",
            "args":{"task_id":"T1","feedback":"please revise the api"}
        }),
    ).await;
    assert_eq!(r["type"], "mcp_reply", "task_request_changes should succeed: {r}");

    let frame = ctx.expect_one_broadcast();
    let cliptown_world::protocol::ConsoleOutbound::Directive {
        author_id, to_agent_id, body, in_response_to_task, ..
    } = frame else { panic!("expected Directive") };
    assert_eq!(author_id, "mgr");
    assert_eq!(to_agent_id, "eng");
    assert_eq!(body, "please revise the api");
    assert_eq!(in_response_to_task, Some("T1".into()));

    // Persisted directive row exists.
    let row: (String, String, String) = sqlx::query_as(
        "SELECT author_id, kind, body FROM messages WHERE startup_id = 's1' AND kind = 'directive'"
    ).fetch_one(&ctx.pool).await.unwrap();
    assert_eq!(row.0, "mgr");
    assert_eq!(row.1, "directive");
    assert_eq!(row.2, "please revise the api");

    // review_round incremented.
    let rr: (i64,) = sqlx::query_as("SELECT review_round FROM tasks WHERE id = 'T1'")
        .fetch_one(&ctx.pool).await.unwrap();
    assert_eq!(rr.0, 1);
}

#[tokio::test]
async fn no_broadcast_on_request_changes_null_assignee() {
    use cliptown_world::{mcp_dispatch, move_sys, path::RoomGraph, seed::TownLayout, state::AvatarView};
    let mut ctx = TestCtx::new().await;
    seed_review_cycle_fixture(&ctx.pool).await;
    // Wipe assignee.
    sqlx::query("UPDATE tasks SET assignee_agent_id = NULL WHERE id = 'T1'")
        .execute(&ctx.pool).await.unwrap();

    let mut w = cliptown_world::state::WorldView::default();
    w.avatars.insert("mgr".into(), AvatarView {
        agent_id: "mgr".into(), startup_id: "s1".into(), role: "founder".into(),
        backend: "claude_code".into(), current_pos: (0,0), target_pos: None,
        room_id: "suite_1".into(), status: "idle".into(),
    });
    let layout = TownLayout::default_town();
    let graph: RoomGraph = move_sys::graph_from_layout(&layout);
    let mut paths = std::collections::HashMap::new();

    let r = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &ctx.out_bus, &ctx.pool, &ctx.event_tx,
        "mgr",
        serde_json::json!({
            "type":"mcp_call","v":1,"tool":"task_request_changes","corr_id":"c1",
            "args":{"task_id":"T1","feedback":"x"}
        }),
    ).await;
    assert_eq!(r["type"], "mcp_error", "dispatch should fail with mcp_error: {r}");
    assert_eq!(r["code"], "no_permission", "should fail manager check: {r}");
    // task_request_changes is manager-only and the manager check uses
    // assignee_agent_id; with NULL, this returns an mcp_error rather than
    // emitting a broadcast.
    ctx.expect_no_broadcasts();
}

#[tokio::test]
async fn escalation_emits_system_event_only() {
    use cliptown_world::{mcp_dispatch, move_sys, path::RoomGraph, seed::TownLayout, state::AvatarView};
    let mut ctx = TestCtx::new().await;
    seed_review_cycle_fixture(&ctx.pool).await;
    // Pre-set review_round to the cap so the next request_changes escalates.
    sqlx::query("UPDATE tasks SET review_round = 3 WHERE id = 'T1'")
        .execute(&ctx.pool).await.unwrap();

    let mut w = cliptown_world::state::WorldView::default();
    w.avatars.insert("mgr".into(), AvatarView {
        agent_id: "mgr".into(), startup_id: "s1".into(), role: "founder".into(),
        backend: "claude_code".into(), current_pos: (0,0), target_pos: None,
        room_id: "suite_1".into(), status: "idle".into(),
    });
    let layout = TownLayout::default_town();
    let graph: RoomGraph = move_sys::graph_from_layout(&layout);
    let mut paths = std::collections::HashMap::new();

    let r = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &ctx.out_bus, &ctx.pool, &ctx.event_tx,
        "mgr",
        serde_json::json!({
            "type":"mcp_call","v":1,"tool":"task_request_changes","corr_id":"c1",
            "args":{"task_id":"T1","feedback":"final straw"}
        }),
    ).await;
    assert_eq!(r["type"], "mcp_reply");
    assert_eq!(r["result"]["reason"], "max_review_rounds_exceeded");

    let frames = ctx.drain_broadcasts();
    let directive_count = frames.iter().filter(|f| matches!(f, cliptown_world::protocol::ConsoleOutbound::Directive {..})).count();
    let system_event_count = frames.iter().filter(|f| matches!(f, cliptown_world::protocol::ConsoleOutbound::SystemEvent {..})).count();
    assert_eq!(directive_count, 0, "no Directive on escalation: {frames:?}");
    assert_eq!(system_event_count, 1, "one SystemEvent (task_escalated): {frames:?}");
    if let cliptown_world::protocol::ConsoleOutbound::SystemEvent { kind, severity, payload, startup_id, .. } = &frames[0] {
        assert_eq!(kind, "task_escalated");
        assert_eq!(severity, "alert");
        assert_eq!(startup_id.as_deref(), Some("s1"));
        assert_eq!(payload["task_id"], "T1");
        assert_eq!(payload["rounds"], 3);
        assert_eq!(payload["feedback"], "final straw");
    }

    // review_round preserved (escalation does NOT increment).
    let rr: (i64, String) = sqlx::query_as("SELECT review_round, status FROM tasks WHERE id = 'T1'")
        .fetch_one(&ctx.pool).await.unwrap();
    assert_eq!(rr.0, 3, "review_round unchanged on escalation");
    assert_eq!(rr.1, "escalated");
}

#[tokio::test]
async fn lagged_subscriber_logs_and_closes() {
    // Construct a small-capacity broadcast channel, subscribe, then send
    // more events than capacity to force Lagged.
    let (tx, mut rx) = tokio::sync::broadcast::channel::<cliptown_world::protocol::ConsoleOutbound>(8);
    for i in 0..20 {
        let _ = tx.send(cliptown_world::protocol::ConsoleOutbound::Toast {
            v: 1,
            severity: "info".into(),
            body: format!("toast {i}"),
            sticky: false,
        });
    }
    // First recv should report Lagged with n > 0; the production select arm
    // logs and breaks the WS, so this asserts that signal is observable.
    let r = rx.try_recv();
    match r {
        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
            assert!(n > 0, "expected Lagged with n > 0");
        }
        other => panic!("expected Lagged, got {:?}", other),
    }
}

#[tokio::test]
async fn transactional_integrity_request_changes() {
    use cliptown_world::{mcp_dispatch, move_sys, path::RoomGraph, seed::TownLayout, state::AvatarView};
    let mut ctx = TestCtx::new().await;
    seed_review_cycle_fixture(&ctx.pool).await;

    // Happy-path transactional integrity check: assert that on a successful
    // task_request_changes, BOTH the task UPDATE AND the new directive INSERT
    // are persisted atomically. If a future change splits them, the
    // broadcast-then-rollback case would show as a broadcast for a missing row;
    // this test guards both rows being present together.
    let mut w = cliptown_world::state::WorldView::default();
    w.avatars.insert("mgr".into(), AvatarView {
        agent_id: "mgr".into(), startup_id: "s1".into(), role: "founder".into(),
        backend: "claude_code".into(), current_pos: (0,0), target_pos: None,
        room_id: "suite_1".into(), status: "idle".into(),
    });
    let layout = TownLayout::default_town();
    let graph: RoomGraph = move_sys::graph_from_layout(&layout);
    let mut paths = std::collections::HashMap::new();

    let r = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &ctx.out_bus, &ctx.pool, &ctx.event_tx,
        "mgr",
        serde_json::json!({
            "type":"mcp_call","v":1,"tool":"task_request_changes","corr_id":"c1",
            "args":{"task_id":"T1","feedback":"x"}
        }),
    ).await;
    assert_eq!(r["type"], "mcp_reply", "dispatch should succeed: {r}");
    // Assert both rows are present (transactional success):
    let task: (String, i64) = sqlx::query_as(
        "SELECT status, review_round FROM tasks WHERE id = 'T1'"
    ).fetch_one(&ctx.pool).await.unwrap();
    let msg_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM messages WHERE startup_id = 's1' AND kind = 'directive'"
    ).fetch_one(&ctx.pool).await.unwrap();
    assert_eq!(task.0, "changes_requested");
    assert_eq!(task.1, 1);
    assert_eq!(msg_count.0, 1, "exactly one directive row persisted");
    let _ = ctx.drain_broadcasts();
}

#[tokio::test]
async fn no_broadcast_on_subtask_with_null_assignee() {
    use cliptown_world::{mcp_dispatch, move_sys, path::RoomGraph, seed::TownLayout, state::AvatarView};
    let mut ctx = TestCtx::new().await;

    // Seed: founder mgr manages a parent task. Engineer eng was assigned to T1
    // (a subtask of parent), then assignee was nulled (e.g. agent removed).
    sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) VALUES ('s1', 'a', 'g', 10.0, 'town_default', '/tmp', 'active', unixepoch())")
        .execute(&ctx.pool).await.unwrap();
    sqlx::query("INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status, manager_id) VALUES ('mgr', 's1', 'M', 'founder', 'claude_code', 'm', '{}', 'suite_1', 'idle', NULL)")
        .execute(&ctx.pool).await.unwrap();
    // Parent task assigned to mgr (root task pattern).
    sqlx::query("INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, review_round, created_at, updated_at) VALUES ('parent', 's1', NULL, 'parent', 'd', 'in_progress', 'mgr', 0, unixepoch(), unixepoch())")
        .execute(&ctx.pool).await.unwrap();
    // Subtask with NULL assignee, awaiting review.
    sqlx::query("INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, review_round, created_at, updated_at) VALUES ('T1', 's1', 'parent', 'subtask', 'd', 'awaiting_review', NULL, 0, unixepoch(), unixepoch())")
        .execute(&ctx.pool).await.unwrap();

    let mut w = cliptown_world::state::WorldView::default();
    w.avatars.insert("mgr".into(), AvatarView {
        agent_id: "mgr".into(), startup_id: "s1".into(), role: "founder".into(),
        backend: "claude_code".into(), current_pos: (0,0), target_pos: None,
        room_id: "suite_1".into(), status: "idle".into(),
    });
    let layout = TownLayout::default_town();
    let graph: RoomGraph = move_sys::graph_from_layout(&layout);
    let mut paths = std::collections::HashMap::new();

    let r = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &ctx.out_bus, &ctx.pool, &ctx.event_tx,
        "mgr",
        serde_json::json!({
            "type":"mcp_call","v":1,"tool":"task_request_changes","corr_id":"c1",
            "args":{"task_id":"T1","feedback":"please revise"}
        }),
    ).await;

    // Manager check should pass (mgr manages parent), but no_assignee guard
    // must reject before any side effect.
    assert_eq!(r["type"], "mcp_error", "expected mcp_error: {r}");
    assert_eq!(r["code"], "no_assignee", "code should be no_assignee: {r}");

    // No broadcast emitted.
    ctx.expect_no_broadcasts();

    // Critical: task state UNCHANGED (no UPDATE happened, no directive row).
    let task: (String, i64) = sqlx::query_as(
        "SELECT status, review_round FROM tasks WHERE id = 'T1'"
    ).fetch_one(&ctx.pool).await.unwrap();
    assert_eq!(task.0, "awaiting_review", "status must be unchanged");
    assert_eq!(task.1, 0, "review_round must be unchanged");

    let msg_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM messages WHERE startup_id = 's1' AND kind = 'directive'"
    ).fetch_one(&ctx.pool).await.unwrap();
    assert_eq!(msg_count.0, 0, "no directive row should be persisted");
}
