# M5 Chat / Directive / SystemEvent Emit — Design Spec

| Field | Value |
|---|---|
| Date | 2026-05-08 |
| Status | Draft (post-Codex audit, pre-implementation) |
| Milestone | M5 (per Phase 0 plan tasks 5.3 + 5.6) |
| Branch | (TBD on implementation start) |
| Related | `docs/superpowers/specs/2026-05-07-cliptown-design.md` § 11.2, § 11.6 · `docs/superpowers/ship-gate.md` · `docs/superpowers/plans/2026-05-07-cliptown-phase-0.md` § Milestone 5 |

## 1. Goal

Lift the last two UI-amenable Phase 1 ship-gate invariants by giving the operator console a live view of chat/directive traffic and previously-dark `system_events`. Today the world emits `WorldViewSnapshot` for state, `WorkerOutbound::*` to specific worker channels, and persists `messages` + `system_events` rows. The operator's `ConsoleOutbound` channel only carries snapshots and the existing backend_catalog; everything else is dark. Frontend reducer cases for `chat`, `directive`, and `system_event` already exist (`store.ts:322`, `store.ts:360-389`), forward-compat scaffolding waiting on this milestone.

After this PR:

- `§ 11.2` (operator directive → manager subtask → engineer assignment) becomes UI-observable: directive in chat panel, queued task in kanban, in-progress with assignee.
- `§ 11.6` (review-cycle round increment + max-rounds escalation) becomes UI-observable: per-round directive with `in_response_to_task`, `review_round` visible on kanban card, `task_escalated` SystemEvent on max-rounds breach.
- The TopBar event feed (M4.3) becomes functional for the first time. Budget warnings, permission violations, worker_dead, task_escalated, startup_dissolved all reach the operator live.

## 2. Scope

### In scope (this PR)

1. New `tokio::sync::broadcast::Sender<ConsoleOutbound>` channel (capacity 4096).
2. Two new `ConsoleOutbound` variants: `Chat`, `Directive`.
3. Four chat/directive emit sites with proper guards.
4. New `crates/world/src/emit.rs` module with `emit_system_event` helper that owns id+ts and broadcasts.
5. Migration of all 5 `persist::record_system_event` callers to `emit_system_event`.
6. `review_round` + `max_review_rounds` (sourced from `config::TaskCfg`) added to TaskVM projection in `build_console_snapshot`.
7. `handle_console` subscribes to broadcast; `Lagged` is fatal-close (operator's frontend reconnects to a fresh snapshot).
8. Frontend: `TaskVM` extension, severity union extended with `"critical"`, reducer reads `m.message_id ?? m.id`, dedup by id.
9. Tests: new `tests/console_emit.rs` for unit-style emit assertions, extensions to `e2e_directive_chain.rs` / `e2e_review_cycle.rs` / `budget_thresholds.rs` for caller-path coverage, 53 existing test call sites migrated via a shared fixture helper.

### Out of scope (follow-up PRs, named explicitly)

- Phase 1 § 11.2 + § 11.6 Playwright tests themselves (separate PR per ship-gate cadence).
- `recent_messages` backfill on WS reconnect — chat/directive history isn't rehydrated; reconnecting operator sees forward-only.
- `messages.recipient_agent_id` SQL column — directives are persisted without their recipient. Live broadcast carries it; SQL replay loses it. Aligned with no-backfill scope above.
- `ConsoleOutbound::Toast` and `Modal` variants are dark wires too (zero emitters). Same pattern, not addressed here.
- `handle_speak` directive-permission denial doesn't emit `permission_violation` — small symmetry gap, one-line cleanup post-Gap-B but deferred.
- `WorkerOutbound::Directive` adding `in_response_to_task` field consistency — protocol drift between worker-side and console-side directive shapes. Defer.
- `operator_chat` ConsoleInbound variant — `ChatPanel` has a dead send path with no inbound handler. Preexisting; not introduced or fixed here.

## 3. Architecture overview

### 3.1 New channel

`tokio::sync::broadcast::Sender<ConsoleOutbound>` with capacity 4096. Lives on `Handle` (already passed via `AppState`). `handle_console` subscribes via `event_tx.subscribe()` per WS connection. Capacity 4096 makes `Lagged(n)` practically unreachable at Phase-0 single-startup load (one chat + one directive per few seconds vs 4096 buffered), and on the unlikely case of a true lag we log a warning and close the WS so the frontend reconnects to a fresh snapshot.

### 3.2 New protocol variants

`crates/world/src/protocol/ws_messages.rs` adds two `ConsoleOutbound` variants exported via `ts-rs`:

```rust
ConsoleOutbound::Chat {
    v: u8,
    message_id: String,    // uuid; matches messages.id
    ts: i64,               // UNIX MILLISECONDS (see § 3.3)
    startup_id: String,
    room_id: String,       // chat is always room-scoped
    author_id: String,     // real agent_id; never sentinel
    body: String,
}

ConsoleOutbound::Directive {
    v: u8,
    message_id: String,
    ts: i64,               // UNIX MILLISECONDS
    startup_id: String,
    author_id: String,     // sentinel "operator" for operator-sourced; agent_id otherwise
    to_agent_id: String,   // recipient (validated non-empty before emit)
    body: String,
    in_response_to_task: Option<String>,  // Some(task_id) only for review-cycle feedback
}
```

### 3.3 Timestamp unit decision

**Wire format is UNIX milliseconds.** Codex audit caught a real bug: existing SQL writes use `unixepoch()` (seconds), but `ChatPanel` and `HistoryModal` render `new Date(m.ts)` which expects milliseconds. Without correction, every new frame would display as a 1970 date.

Decision: protocol `ts` field carries milliseconds. SQL stays in seconds. Conversion happens at the emit site:

```rust
let ts_ms: i64 = chrono::Utc::now().timestamp_millis();
// or, when reading back from SQL after INSERT: row_ts_secs * 1000
```

Existing SystemEvent `ts` field (currently undocumented unit) gets the same treatment via the new `emit_system_event` helper.

### 3.4 Frontend changes

- `TaskVM` gains optional `review_round?: number`, `max_review_rounds?: number` (`store.ts:44-51`).
- `SystemEventVM["severity"]` union extended: `"info" | "warn" | "alert" | "critical"`. `severityFromString` updated to recognize `critical`. (Caught by Codex: SQL CHECK constraint and protocol both allow `critical` but frontend silently downgraded.)
- `store.ts:369` reducer reads `m.message_id ?? m.id` so new protocol field works without breaking synthetic-frame tests using `id`.
- Reducer adds dedup-by-id on chat/directive append: skip if `state.messages.find(x => x.id === incoming.id)`. Costs O(N) per append but prevents future double-emission or retry-storm dupes.

## 4. Emit sites

### 4.1 Invariant

Every broadcast happens **after the SQL write succeeds**. SQL failure → no broadcast. The `handle_task_request_changes` path uses a SQL transaction (per § 4.4 below) so the partial-write window is closed.

### 4.2 E1 — `cmd_console::OperatorDirective` (operator → agent)

Today: validates → INSERTs into messages → pushes to recipient `out_bus` → returns reply.

**Change:** pre-fetch recipient validity and startup_id in a separate SELECT *before* any side effect (Codex M4):

```rust
let row: Option<(String,)> = sqlx::query_as("SELECT startup_id FROM agents WHERE id = ?")
    .bind(&to_agent_id).fetch_optional(pool).await
    .map_err(|e| ("sql".into(), e.to_string()))?;
let recipient_startup_id = match row {
    Some((sid,)) => sid,
    None => return json!({"type":"error","reason":"unknown_recipient"}),
};
// Then: INSERT (no inline subquery), out_bus push, broadcast.
```

After successful INSERT, broadcast:

```rust
let _ = event_tx.send(ConsoleOutbound::Directive {
    v: 1,
    message_id: id.clone(),
    ts: chrono::Utc::now().timestamp_millis(),
    startup_id: recipient_startup_id,
    author_id: "operator".into(),
    to_agent_id: to_agent_id.clone(),
    body: body.clone(),
    in_response_to_task: None,
});
```

`let _ =` discards the `Result` — `Err` only when zero subscribers exist (no operator connected), which is not an error.

### 4.3 E2 — `mcp_dispatch::handle_speak` (peer chat or peer directive)

Today: validates → INSERTs into messages (room_id set for chat, NULL for directive) → fans to peer `out_bus`(es).

**Change:** after successful INSERT, broadcast either Chat or Directive based on `kind`:

```rust
let frame = if kind == "chat" {
    ConsoleOutbound::Chat {
        v: 1, message_id: id.clone(), ts: chrono::Utc::now().timestamp_millis(),
        startup_id: caller.startup_id.clone(),
        room_id: caller.room_id.clone(),
        author_id: caller.agent_id.clone(),
        body: body.clone(),
    }
} else {
    // Directive — to_agent_id already validated above (line 363-365)
    ConsoleOutbound::Directive {
        v: 1, message_id: id.clone(), ts: chrono::Utc::now().timestamp_millis(),
        startup_id: caller.startup_id.clone(),
        author_id: caller.agent_id.clone(),
        to_agent_id: to_agent_id.clone().unwrap(),
        body: body.clone(),
        in_response_to_task: None,
    }
};
let _ = event_tx.send(frame);
```

### 4.4 E3 — `mcp_dispatch::handle_task_request_changes`

This handler has TWO branches: regular round-increment, and max-rounds escalation. Both need the design pinned.

#### 4.4.1 Regular round-increment branch

Today: validates → UPDATEs task (status + review_round++) → audit row → push assignee `out_bus`. **Does NOT persist directive to messages.**

**Change:** wrap task UPDATE + new directive INSERT into a single SQL transaction (Codex B4 — eliminates the contradiction between "best-effort INSERT" and the broadcast-after-SQL invariant):

```rust
let mut tx = pool.begin().await.map_err(...)?;
sqlx::query("UPDATE tasks SET status = ?, review_round = review_round + 1, updated_at = unixepoch() WHERE id = ?")
    .bind(status_to_str(new_status)).bind(&task_id)
    .execute(&mut *tx).await.map_err(...)?;
sqlx::query("INSERT INTO messages (id, startup_id, room_id, author_id, body, kind, ts) VALUES (?, ?, NULL, ?, ?, 'directive', unixepoch())")
    .bind(&directive_id).bind(&caller.startup_id).bind(&caller.agent_id).bind(&feedback)
    .execute(&mut *tx).await.map_err(...)?;
tx.commit().await.map_err(...)?;
```

After commit:
- `persist::append_audit` (existing fire-and-forget pattern stays)
- Broadcast Directive frame **only if** `task.assignee_agent_id.is_some()` (Codex M4 — current code skips out_bus on None, broadcast must mirror):
  ```rust
  if let Some(assignee) = task.assignee_agent_id.as_deref() {
      let _ = event_tx.send(ConsoleOutbound::Directive {
          v: 1, message_id: directive_id, ts: chrono::Utc::now().timestamp_millis(),
          startup_id: caller.startup_id.clone(),
          author_id: caller.agent_id.clone(),
          to_agent_id: assignee.to_string(),
          body: feedback.clone(),
          in_response_to_task: Some(task_id.clone()),
      });
      // Existing out_bus push to assignee stays as-is.
  }
  ```

#### 4.4.2 Escalation branch (max_review_rounds breach)

Today: validates → calls `next(task.status, &Transition::Escalate)` → UPDATE status → audit_trail with `max_review_rounds_exceeded` → `persist::record_system_event` with kind `task_escalated`.

**Change:** swap `record_system_event` for `emit_system_event` (§ 5). No directive INSERT, no Directive broadcast in this branch.

**Negation tests required (Codex B5):**
- No Directive frame on escalation
- `review_round` field UNCHANGED in post-state snapshot (escalation doesn't increment)
- Task status = `escalated` in snapshot
- Exactly one SystemEvent with `kind: "task_escalated"`, `severity: "alert"`, `payload.task_id`, `payload.rounds`, `payload.feedback`

### 4.5 E4 — `emit_system_event` helper (Gap B)

New module `crates/world/src/emit.rs`:

```rust
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use serde_json::Value;
use crate::protocol::ConsoleOutbound;

pub async fn emit_system_event(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<ConsoleOutbound>,
    startup_id: Option<&str>,
    kind: &str,
    payload: &str,    // JSON string, matches existing record_system_event signature
    severity: &str,
) -> Result<(), sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let ts_secs = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO system_events (id, startup_id, kind, payload, severity, ts) VALUES (?, ?, ?, ?, ?, ?)"
    )
    .bind(&id).bind(startup_id).bind(kind).bind(payload).bind(severity).bind(ts_secs)
    .execute(pool).await?;
    let _ = event_tx.send(ConsoleOutbound::SystemEvent {
        v: 1,
        severity: severity.into(),
        kind: kind.into(),
        startup_id: startup_id.map(String::from),
        payload: serde_json::from_str(payload).unwrap_or(Value::Null),
        ts: ts_secs * 1000,  // milliseconds on the wire
    });
    Ok(())
}
```

This addresses Codex B3: helper *owns* both id and ts, so the broadcast frame and SQL row carry identical values. `record_system_event` stays as a low-level SQL helper but gets a deprecation comment pointing at `emit_system_event` for new callers.

Migrate the 5 callers:

| File | Line | Event | Note |
|---|---|---|---|
| `budget.rs` | 237 | budget thresholds (80/95/100) | already in scope |
| `agent_supervisor.rs` | 168 | `worker_dead` | needs `event_tx` threading (Codex M3) |
| `mcp_dispatch.rs` | 322 | `permission_violation` (move_intent denial) | event_tx already threaded for E2/E3 |
| `mcp_dispatch.rs` | 810 | `task_escalated` | same |
| `api_startups.rs` | 395 | `startup_dissolved` | event_tx via AppState |

`AgentSupervisor::new` gains an `event_tx: broadcast::Sender<ConsoleOutbound>` parameter; `main.rs` threads it in alongside `pool`.

`api_startups::delete_startup` calls `emit_system_event` directly (it has `AppState`). Codex flagged that the broadcast can race ahead of the world loop's `ReleaseSuite` processing — operator might see "startup_dissolved" before the snapshot reflects it. Acceptable for a one-shot dissolution event; documented in § 8 (known limitations).

## 5. Snapshot projection

`build_console_snapshot` in `http.rs:154-189` extends the tasks SELECT:

```rust
"SELECT id, startup_id, title, status, assignee_agent_id, required_room, review_round FROM tasks"
```

Each task object adds:

```rust
"review_round": <SQL value>,           // u32 from tasks.review_round column
"max_review_rounds": <config_value>,   // i.e. cfg.task.max_review_rounds
```

`max_review_rounds` is sourced from `config::TaskCfg.max_review_rounds`, NOT from the private `MAX_REVIEW_ROUNDS` const in `mcp_dispatch.rs` (Codex B2). The config is already loaded into `AppState` indirectly; we expose `cfg.task.max_review_rounds` via `AppState` for `build_console_snapshot` to read. Bonus: `mcp_dispatch.rs` should switch from the const to config-sourced too (matches the existing M9-hardening TODO at `mcp_dispatch.rs:34`). Whether that switch happens *in this PR* or as a sibling cleanup is a small judgment call; this spec says *in this PR* for consistency.

## 6. Threading

```
main.rs
  ├─ creates broadcast::channel(4096) → (event_tx, _)
  ├─ AgentSupervisor::new(pool, event_tx.clone(), …)
  ├─ loop_::spawn_with_layout(pool, event_tx.clone(), …)
  └─ AppState { pool, handle, supervisor, event_tx, … }

loop_::spawn_with_layout
  ├─ owns event_tx (cloned into the spawned task)
  └─ Cmd::HandleConsoleMsg → cmd_console::dispatch(world, pool, out_bus, &event_tx, msg)
       Cmd::HandleWorkerMsg → cmd_worker::dispatch(…, &event_tx, …)
                                  └─ mcp_dispatch::dispatch(…, &event_tx, …)

handle_console (http.rs)
  ├─ event_rx = state.event_tx.subscribe();
  └─ select! {
       inbound = receiver.next() => …
       changed = view_rx.changed() => …
       msg = event_rx.recv() => match msg {
         Ok(frame) => sender.send(json),
         Err(Lagged(n)) => {
           tracing::warn!(component = "handle_console", lagged = n,
             "console lagged; closing WS to force resync");
           break;  // frontend reconnects, gets fresh snapshot
         }
         Err(Closed) => break,
       }
     }
```

`Handle` struct gains `event_tx` field (Clone-friendly) so `AppState::handle.event_tx.subscribe()` works in `handle_console`.

## 7. Testing

### 7.1 New `tests/console_emit.rs`

| Test | Asserts |
|---|---|
| `broadcasts_on_operator_directive` | One Directive frame, author="operator", correct startup_id resolved via prefetch, message_id matches SQL row |
| `no_broadcast_on_unknown_recipient` | `OperatorDirective` to unknown agent_id returns `unknown_recipient` error AND zero broadcast frames |
| `broadcasts_on_peer_chat` | One Chat frame, room_id set, author = caller |
| `broadcasts_on_peer_directive` | One Directive frame, in_response_to_task = None |
| `broadcasts_on_review_request_changes` | One Directive frame with `in_response_to_task: Some(task_id)`; messages SQL has the persisted directive row; both committed atomically |
| `no_broadcast_on_request_changes_null_assignee` | Skip broadcast when assignee is None |
| `escalation_emits_system_event_only` | Max-rounds path: one SystemEvent (kind=task_escalated, severity=alert), zero Directive frames, review_round unchanged, task status=escalated |
| `transactional_integrity` | Force INSERT into messages to fail (e.g., truncate body to a constraint-violating length); assert task UPDATE rolled back AND zero broadcasts |
| `lagged_subscriber_logs_and_closes` | Send 5000 events to a connected receiver, assert it sees Lagged then Closed (drives the fatal-close design) |

### 7.2 Extend existing tests

- `e2e_directive_chain.rs`: subscribe to event_tx; after step 1, assert one Directive frame.
- `e2e_review_cycle.rs`: assert per-round Directive (in_response_to_task=Some); messages count +1; max-rounds path asserts SystemEvent emitted via emit_system_event.
- `view_chunk.rs` (or wherever `build_console_snapshot` is exercised): assert `review_round` + `max_review_rounds` present in tasks projection.
- `budget_thresholds.rs`: extend each threshold-crossing test to subscribe and assert the SystemEvent broadcast (caller-path coverage per Codex M16).
- `e2e_permission_violation.rs`: assert SystemEvent broadcast for `permission_violation`.
- A new `agent_supervisor` test that exercises the worker_dead path and asserts SystemEvent.
- `api_startups.rs` tests: assert `startup_dissolved` SystemEvent on DELETE.

### 7.3 Test-fixture helper for the 53-call-site migration

Add `crates/world/tests/common/dispatch_ctx.rs` (or similar) that bundles `pool`, `out_bus`, `event_tx`, plus a helper `assert_no_broadcasts(rx)` and `expect_one_broadcast<T>(rx) -> T`. This addresses Codex M18: 53 tests passing throwaway senders is a regression-coverage liability; the fixture makes "I expected this emit, did I get it?" the default question.

Migration mechanic per test:

```rust
let ctx = TestCtx::new().await;   // creates pool + out_bus + event_tx + receiver
let result = cmd_console::dispatch(&mut w, &ctx.pool, &ctx.out_bus, &ctx.event_tx, msg).await;
ctx.expect_no_broadcasts();  // for tests not asserting broadcasts
```

## 8. Known limitations (documented, not fixed)

| Limitation | Why accepted | Future fix path |
|---|---|---|
| Broadcast is lossy under sustained load | Capacity 4096 + fatal-close on Lagged means Phase-0 single-startup load won't trigger; if it does, frontend reconnects to a fresh snapshot. Truly chatty multi-tenant load is M5+ work anyway. | Switch to per-console mpsc with backpressure (Approach 3 from brainstorm) when multi-tenant scale demands it |
| `messages` table has no `recipient_agent_id` column | Backfill on reconnect is out of scope (see § 2). Live broadcast carries `to_agent_id`; persistence loses it. | Add column via SQL migration when reconnect-with-history is in scope |
| Reconnecting console misses chat history | Same as above — reconnect backfill is deferred | Hydrate `recent_messages` (last N) in initial snapshot only |
| `api_startups::delete_startup` SystemEvent can race ahead of snapshot reflecting dissolution | One-shot event, semantically OK | Route emission through the loop after `ReleaseSuite` is applied |
| Directives appear in all room views (no `room_id`) | By design — directives are room-independent (matches existing schema) | Add scope filter in ChatPanel if cross-room directive bleed becomes an issue |
| `ChatPanel` has dead `operator_chat` send path | Preexisting; no inbound handler | Add `OperatorChat` ConsoleInbound variant or remove dead UI |
| `WorkerOutbound::Directive` lacks `in_response_to_task` consistency | Preexisting protocol drift | Sync worker and console directive shapes in a follow-up |
| ts-rs generates `bigint` for `i64`/`u64` fields | Preexisting; JSON parses as number; reducers cope structurally | Add WS-boundary normalization or change generation strategy |
| `ConsoleOutbound::Toast` and `Modal` have no emitters | Same dark-wire pattern as Chat/Directive/SystemEvent before this PR | Future milestone |
| Broadcast can arrive before snapshot in single-mutation cases | Eventually-consistent UI; tests use `waitFor` patterns | Restructure dispatch to return broadcast queue, drain after view_tx |
| `handle_speak` directive permission-denial doesn't `record_system_event` | Asymmetry vs `move_intent` denial path | One-line `emit_system_event` call once Gap B lands |

## 9. Migration & rollout

**No SQL migrations required.** All affected columns (`tasks.review_round`, `messages.kind`, `system_events.severity`) already exist.

**Wire-format compatibility.** New `ConsoleOutbound` variants are backward-compatible — old frontends fall through the reducer's `default` arm (`store.ts:391`) and ignore unknown variants. New frontend on old server: existing `chat`/`directive`/`system_event` reducer cases stay quiet. Forward and backward compatible.

**53-test signature migration.** Mechanical: each call site gets a `&ctx.event_tx` (or `_event_tx` if not asserting). The fixture helper from § 7.3 keeps the diff small per file.

## 10. Verification

`cargo test -p cliptown-world` must pass. `pnpm tsc --noEmit` in `packages/frontend` must pass. The existing 9-test Playwright suite (`packages/frontend/e2e/`) must continue passing — the reducer tweak (`m.message_id ?? m.id`) is backward-compatible with synthetic-id dispatches in `ship-gate.spec.ts`.

The Phase 1 § 11.2 + § 11.6 Playwright tests are explicitly a follow-up PR. They get to be small and clean because this PR closes every dark wire they need.
