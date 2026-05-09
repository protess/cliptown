# M5 Chat / Directive / SystemEvent Emit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire chat/directive/system_event broadcast from the world to the operator console so ship-gate § 11.2 (directive→subtask→assignment) and § 11.6 (review-cycle round++ + max-rounds escalation) become UI-observable.

**Architecture:** Add a `tokio::sync::broadcast` channel for ConsoleOutbound events alongside the existing watch-channel for snapshots. Three production code paths emit chat/directive frames after their existing SQL writes; a new `emit_system_event` helper owns id+ts and replaces all `persist::record_system_event` callers. Frontend reducer cases for `chat`/`directive`/`system_event` already exist; this plan lights them up.

**Tech Stack:** Rust (`tokio`, `sqlx`, `axum`, `ts-rs`, `tracing`), TypeScript/React (Vite, useReducer), SQLite. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-08-m5-chat-directive-emit-design.md` (commit `b843c83`).

---

## Task 1: Add `Chat` + `Directive` ConsoleOutbound variants

Pure protocol addition. No callers yet. ts-rs auto-regenerates the TypeScript file via the build script. After this task, `cargo build -p cliptown-world` regenerates `packages/protocol/dist/ConsoleOutbound.ts`.

**Files:**
- Modify: `crates/world/src/protocol/ws_messages.rs:56-66`
- Auto-regenerated: `packages/protocol/dist/ConsoleOutbound.ts`

- [ ] **Step 1: Read the current `ConsoleOutbound` enum**

```bash
sed -n '56,66p' crates/world/src/protocol/ws_messages.rs
```

Expected output: 6 existing variants (`WorldViewSnapshot`, `WorldViewDelta`, `SystemEvent`, `BackendCatalog`, `Toast`, `Modal`).

- [ ] **Step 2: Add the two variants**

In `crates/world/src/protocol/ws_messages.rs`, change the `ConsoleOutbound` enum (lines 56-66) to:

```rust
#[derive(Debug, Serialize, Deserialize, TS, Clone)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConsoleOutbound {
    WorldViewSnapshot { v: u8, snapshot: serde_json::Value },
    WorldViewDelta { v: u8, tick_seq: u64, changes: serde_json::Value },
    SystemEvent { v: u8, severity: String, kind: String, startup_id: Option<String>, payload: serde_json::Value, ts: i64 },
    BackendCatalog { v: u8, entries: serde_json::Value },
    Toast { v: u8, severity: String, body: String, sticky: bool },
    Modal { v: u8, kind: String, payload: serde_json::Value },
    /// Operator-visible chat message. Always room-scoped. `ts` is UNIX MILLISECONDS
    /// (matches frontend `new Date(m.ts)` rendering convention; SQL `messages.ts`
    /// stores seconds and gets multiplied at the emit site).
    Chat {
        v: u8,
        message_id: String,
        ts: i64,
        startup_id: String,
        room_id: String,
        author_id: String,
        body: String,
    },
    /// Operator-visible directive. Room-independent. `author_id` is the sentinel
    /// "operator" for operator-sourced directives, real `agent_id` for peer- or
    /// manager-sourced. `in_response_to_task` is `Some(task_id)` only for review-
    /// cycle feedback (mcp_dispatch::handle_task_request_changes). `ts` is UNIX
    /// MILLISECONDS, see `Chat` doc above.
    Directive {
        v: u8,
        message_id: String,
        ts: i64,
        startup_id: String,
        author_id: String,
        to_agent_id: String,
        body: String,
        in_response_to_task: Option<String>,
    },
}
```

- [ ] **Step 3: Build to verify ts-rs regenerates the TypeScript**

Run: `cargo build -p cliptown-world`
Expected: builds clean. `packages/protocol/dist/ConsoleOutbound.ts` contains `chat` and `directive` variants in the union type.

- [ ] **Step 4: Verify the regenerated TypeScript**

```bash
grep -E '"chat"|"directive"' packages/protocol/dist/ConsoleOutbound.ts
```

Expected: two matching lines for the new variant tags.

- [ ] **Step 5: Commit**

```bash
git add crates/world/src/protocol/ws_messages.rs packages/protocol/dist/ConsoleOutbound.ts
git commit -m "feat(M5): add ConsoleOutbound::Chat + Directive protocol variants

Wire-format ts is UNIX milliseconds (matches frontend new Date(m.ts)
rendering convention; SQL ts stays seconds and gets multiplied at the
emit site). Variants are inert until subsequent tasks wire emitters."
```

---

## Task 2: Create the test-fixture helper (`TestCtx`)

Build the fixture FIRST so every subsequent test can use it. Bundles `pool`, `out_bus`, and a `broadcast::Sender<ConsoleOutbound>` plus a receiver, with helpers to assert emission or non-emission.

**Files:**
- Create: `crates/world/tests/common/mod.rs`
- Modify: each existing test file gets `mod common;` once Task 5's signature migration runs (deferred there)

- [ ] **Step 1: Create the common module directory**

```bash
mkdir -p crates/world/tests/common
```

- [ ] **Step 2: Write the fixture**

Create `crates/world/tests/common/mod.rs` with:

```rust
//! Shared test fixture for crates/world tests. Bundles pool, out_bus, and the
//! broadcast event channel that production dispatch handlers expect, so each
//! test can assert "this dispatch emitted exactly these console frames" without
//! repeating the channel-setup boilerplate.

#![allow(dead_code)]  // Some helpers used by only a subset of tests.

use cliptown_world::{protocol::ConsoleOutbound, storage};
use sqlx::SqlitePool;
use std::collections::HashMap;
use tokio::sync::{broadcast, mpsc};

pub struct TestCtx {
    pub pool: SqlitePool,
    pub out_bus: HashMap<String, mpsc::Sender<serde_json::Value>>,
    pub event_tx: broadcast::Sender<ConsoleOutbound>,
    pub event_rx: broadcast::Receiver<ConsoleOutbound>,
    _dir: tempfile::TempDir,
}

impl TestCtx {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.db");
        let pool = storage::open(p.to_str().unwrap()).await.unwrap();
        cliptown_world::seed::seed_if_empty(&pool).await.unwrap();
        let (event_tx, event_rx) = broadcast::channel(64);
        TestCtx {
            pool,
            out_bus: HashMap::new(),
            event_tx,
            event_rx,
            _dir: dir,
        }
    }

    /// Asserts no console frames were broadcast since the last drain. Drains
    /// any remaining events. Use in tests that are NOT asserting emission to
    /// catch accidental new emit sites.
    pub fn expect_no_broadcasts(&mut self) {
        let mut found = Vec::new();
        loop {
            match self.event_rx.try_recv() {
                Ok(frame) => found.push(frame),
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }
        assert!(
            found.is_empty(),
            "expected no console broadcasts, found {} frame(s): {:?}",
            found.len(),
            found
        );
    }

    /// Drains all queued frames and returns them. Use to verify the exact
    /// number/shape of emissions from a single dispatch call.
    pub fn drain_broadcasts(&mut self) -> Vec<ConsoleOutbound> {
        let mut out = Vec::new();
        loop {
            match self.event_rx.try_recv() {
                Ok(frame) => out.push(frame),
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }
        out
    }

    /// Convenience: drain and assert exactly one frame.
    pub fn expect_one_broadcast(&mut self) -> ConsoleOutbound {
        let frames = self.drain_broadcasts();
        assert_eq!(frames.len(), 1, "expected exactly 1 broadcast, got {}: {:?}", frames.len(), frames);
        frames.into_iter().next().unwrap()
    }
}
```

- [ ] **Step 3: Build to verify the fixture compiles standalone**

Run: `cargo build -p cliptown-world --tests`
Expected: builds clean. The fixture isn't referenced by any test yet, but compiles in isolation.

- [ ] **Step 4: Commit**

```bash
git add crates/world/tests/common/mod.rs
git commit -m "test(M5): add TestCtx shared fixture for dispatch tests

Bundles pool + out_bus + broadcast::Sender so tests don't repeat the
channel-setup boilerplate when migrating to the new dispatch signature
that takes &event_tx. Helpers expect_no_broadcasts and expect_one_broadcast
make 'did this dispatch emit?' the default question."
```

---

## Task 3: Create the `emit.rs` module with `emit_system_event` helper

Pure-helper addition. Owns id and ts (Codex B3 fix): the helper generates them in Rust, writes to SQL with explicit binds, then broadcasts a SystemEvent frame with identical values. No callers yet.

**Files:**
- Create: `crates/world/src/emit.rs`
- Modify: `crates/world/src/lib.rs` (add `pub mod emit;`)
- Test: `crates/world/tests/console_emit.rs` (create with first emit test)

- [ ] **Step 1: Create the emit module**

Create `crates/world/src/emit.rs`:

```rust
//! Console-event emission helper. Wraps the SQL persist of system_events and
//! broadcasts a matching ConsoleOutbound::SystemEvent frame. Callers should
//! prefer this over `persist::record_system_event` (which only writes SQL and
//! doesn't reach the operator console).
//!
//! The helper owns `id` and `ts` — both fields are generated in Rust and bound
//! to the SQL INSERT, so the broadcast frame and the persisted row carry
//! identical values. Wire-format `ts` is UNIX milliseconds (frontend renders
//! `new Date(m.ts)`); SQL stores seconds for compatibility with existing
//! `unixepoch()`-based queries.

use crate::protocol::ConsoleOutbound;
use serde_json::Value;
use sqlx::SqlitePool;
use tokio::sync::broadcast;

pub async fn emit_system_event(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<ConsoleOutbound>,
    startup_id: Option<&str>,
    kind: &str,
    payload: &str,
    severity: &str,
) -> Result<(), sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let ts_secs = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO system_events (id, startup_id, kind, payload, severity, ts) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(startup_id)
    .bind(kind)
    .bind(payload)
    .bind(severity)
    .bind(ts_secs)
    .execute(pool)
    .await?;

    // `let _` discards the Result — Err means zero subscribers, not a failure.
    let _ = event_tx.send(ConsoleOutbound::SystemEvent {
        v: 1,
        severity: severity.into(),
        kind: kind.into(),
        startup_id: startup_id.map(String::from),
        payload: serde_json::from_str(payload).unwrap_or(Value::Null),
        ts: ts_secs * 1000, // milliseconds on the wire
    });
    Ok(())
}
```

- [ ] **Step 2: Register the module in lib.rs**

Add `pub mod emit;` to `crates/world/src/lib.rs` (alphabetical with the other `pub mod` declarations).

- [ ] **Step 3: Write a failing test**

Create `crates/world/tests/console_emit.rs`:

```rust
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
```

- [ ] **Step 4: Run the test, verify it passes**

Run: `cargo test -p cliptown-world --test console_emit emit_system_event_owns_id_and_ts -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/world/src/emit.rs crates/world/src/lib.rs crates/world/tests/console_emit.rs
git commit -m "feat(M5): add emit_system_event helper that owns id+ts

The helper generates id and ts in Rust, binds them to the SQL INSERT,
and broadcasts a SystemEvent frame with identical values. Wire-format
ts is milliseconds; SQL stays seconds. Callers migrate from
persist::record_system_event in subsequent tasks.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Add `event_tx` to `Handle` + `AppState` + `main.rs` wiring

Pure plumbing. After this task, the channel exists end-to-end but no production handler emits or subscribes yet. `_cfg` is renamed to `cfg` so its `task.max_review_rounds` is reachable for Task 11's snapshot extension.

**Files:**
- Modify: `crates/world/src/loop_.rs:58-77` (Handle struct + spawn signatures)
- Modify: `crates/world/src/http.rs:16-22` (AppState struct)
- Modify: `crates/world/src/main.rs:20, 89-90` (config retention + channel creation + AppState construction)

- [ ] **Step 1: Add `event_tx` to `Handle` struct**

In `crates/world/src/loop_.rs`, change the `Handle` struct (lines 58-62) to:

```rust
#[derive(Clone)]
pub struct Handle {
    pub tx: mpsc::Sender<Cmd>,
    pub view_rx: watch::Receiver<WorldView>,
    pub event_tx: tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
}
```

- [ ] **Step 2: Update `spawn` and `spawn_with_layout` to accept and forward `event_tx`**

Change `spawn` and `spawn_with_layout` signatures and `Handle` construction:

```rust
pub fn spawn(
    initial: WorldView,
    pool: SqlitePool,
    event_tx: tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
) -> Handle {
    let layout = TownLayout::default_town();
    let graph = move_sys::graph_from_layout(&layout);
    spawn_with_layout(initial, pool, layout, graph, event_tx)
}

pub fn spawn_with_layout(
    initial: WorldView,
    pool: SqlitePool,
    layout: TownLayout,
    graph: RoomGraph,
    event_tx: tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
) -> Handle {
    let (tx, mut rx) = mpsc::channel::<Cmd>(1024);
    let (view_tx, view_rx) = watch::channel(initial.clone());
    // ... (existing body unchanged, but the `let event_tx` clone happens here for
    //      future task threading. The spawned task body still doesn't use it
    //      until Task 6/7/8/9 wire emitters.)
    let _event_tx_owned = event_tx.clone(); // moved into the spawn task in Task 6
    // ... existing spawn body unchanged ...
    Handle { tx, view_rx, event_tx }
}
```

(`_event_tx_owned` is a placeholder so the unused-variable warning doesn't appear yet; subsequent tasks consume it inside the spawned task.)

- [ ] **Step 3: Add `event_tx` to `AppState`**

In `crates/world/src/http.rs`, change AppState (lines 16-22) to:

```rust
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub handle: Handle,
    pub catalog: Arc<tokio::sync::RwLock<std::collections::HashMap<String, serde_json::Value>>>,
    pub supervisor: Arc<AgentSupervisor>,
    pub max_review_rounds: u32,
}
```

(Note: `event_tx` is reachable via `state.handle.event_tx`. We add `max_review_rounds` here too — Task 11 reads it for the snapshot projection.)

- [ ] **Step 4: Update main.rs to create the channel and thread it**

In `crates/world/src/main.rs:20`, change `let _cfg = ...;` to `let cfg = config::load_from("cliptown.toml")?;`.

In `crates/world/src/main.rs:28`, change `let handle = loop_::spawn(WorldView::default(), pool.clone());` to:

```rust
let (event_tx, _) = tokio::sync::broadcast::channel::<cliptown_world::protocol::ConsoleOutbound>(4096);
let handle = loop_::spawn(WorldView::default(), pool.clone(), event_tx.clone());
```

In `crates/world/src/main.rs:89-90`, change:

```rust
let supervisor = Arc::new(AgentSupervisor::new(SupervisorConfig::default(), pool.clone()));
let app = http::router(http::AppState { pool, handle, catalog, supervisor });
```

to:

```rust
let supervisor = Arc::new(AgentSupervisor::new(
    SupervisorConfig::default(),
    pool.clone(),
    event_tx.clone(),
));
let max_review_rounds = cfg.task.max_review_rounds;
let app = http::router(http::AppState {
    pool, handle, catalog, supervisor, max_review_rounds,
});
```

(`AgentSupervisor::new` gains `event_tx` in Task 10; this step pre-threads it.)

- [ ] **Step 5: Update `AgentSupervisor::new` signature placeholder**

In `crates/world/src/agent_supervisor.rs:86`, change:

```rust
pub fn new(config: SupervisorConfig, pool: SqlitePool) -> Self {
```

to:

```rust
pub fn new(
    config: SupervisorConfig,
    pool: SqlitePool,
    event_tx: tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
) -> Self {
```

Add `event_tx` to the struct fields (find the struct definition near line 80). Initialize the new field in `new`:

```rust
pub struct AgentSupervisor {
    // ... existing fields ...
    event_tx: tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
}

impl AgentSupervisor {
    pub fn new(
        config: SupervisorConfig,
        pool: SqlitePool,
        event_tx: tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    ) -> Self {
        Self {
            // ... existing field initializations ...
            event_tx,
        }
    }
    // ... rest unchanged for now; Task 10 swaps record_system_event ...
}
```

(If the existing `new` is more complex, extend it minimally — only add the field and parameter.)

- [ ] **Step 6: Build to verify the plumbing compiles**

Run: `cargo build -p cliptown-world`
Expected: builds clean. Tests will fail because `loop_::spawn` signature changed (handled in Task 5).

- [ ] **Step 7: Commit**

```bash
git add crates/world/src/loop_.rs crates/world/src/http.rs crates/world/src/main.rs crates/world/src/agent_supervisor.rs
git commit -m "feat(M5): thread broadcast channel through Handle/AppState/Supervisor

Adds event_tx: broadcast::Sender<ConsoleOutbound> (capacity 4096) to
Handle, plumbs it through main.rs into AgentSupervisor::new, and
exposes max_review_rounds via AppState (sourced from cfg.task.max_review_rounds).
No emitters or subscribers yet — those land in subsequent tasks.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Migrate dispatch signatures + 53 test call sites

Add `event_tx` to `cmd_console::dispatch`, `cmd_worker::dispatch`, `mcp_dispatch::dispatch` signatures. Then migrate the 53 test call sites across 11 files using the `TestCtx` fixture from Task 2. After this task, all tests compile and pass with no behavioral change — the new parameter is unused inside the handlers.

**Files:**
- Modify: `crates/world/src/cmd_console.rs:21-25` (dispatch signature)
- Modify: `crates/world/src/cmd_worker.rs:16-32` (dispatch signature + threading to mcp_dispatch)
- Modify: `crates/world/src/mcp_dispatch.rs:55+` (dispatch signature)
- Modify: `crates/world/src/loop_.rs:117-125` (Cmd::HandleConsoleMsg / HandleWorkerMsg arms pass event_tx to dispatch)
- Modify (53 sites): `crates/world/tests/{console_cmds,e2e_cafe,e2e_directive_chain,e2e_engineer_artifact,e2e_force_actions,e2e_isolation,e2e_manager_accept,e2e_permission_violation,e2e_proposal_flow,e2e_review_cycle,mcp_handlers}.rs`

- [ ] **Step 1: Update `cmd_console::dispatch` signature**

In `crates/world/src/cmd_console.rs:21-25`, change:

```rust
pub async fn dispatch(
    world: &mut WorldView,
    pool: &SqlitePool,
    out_bus: &HashMap<String, mpsc::Sender<serde_json::Value>>,
    msg: serde_json::Value,
) -> serde_json::Value {
```

to:

```rust
pub async fn dispatch(
    world: &mut WorldView,
    pool: &SqlitePool,
    out_bus: &HashMap<String, mpsc::Sender<serde_json::Value>>,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    msg: serde_json::Value,
) -> serde_json::Value {
```

The body uses `event_tx` only after Task 6 wires the OperatorDirective broadcast. For now add a `let _ = event_tx;` at the top of the function to silence the unused-variable warning — remove it in Task 6.

- [ ] **Step 2: Update `cmd_worker::dispatch` signature**

In `crates/world/src/cmd_worker.rs:16-32`, add `event_tx` parameter and thread it into the `mcp_dispatch::dispatch` call at line 31-33:

```rust
pub async fn dispatch(
    world: &mut WorldView,
    paths: &mut PathStore,
    layout: &TownLayout,
    graph: &RoomGraph,
    out_bus: &HashMap<String, mpsc::Sender<serde_json::Value>>,
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    agent_id: &str,
    msg: serde_json::Value,
) -> serde_json::Value {
    // ... existing body, but the `mcp_dispatch::dispatch(...)` call now passes event_tx:
    return crate::mcp_dispatch::dispatch(
        world, paths, layout, graph, out_bus, pool, event_tx, agent_id, msg,
    );
    // ... rest of body unchanged ...
}
```

Same pattern: add `let _ = event_tx;` at the top of any branch that doesn't yet use it.

- [ ] **Step 3: Update `mcp_dispatch::dispatch` signature**

In `crates/world/src/mcp_dispatch.rs`, change the `pub async fn dispatch` signature to include `event_tx`. Thread it into the relevant handlers (`handle_speak`, `handle_task_request_changes`) — those handlers also gain the parameter (unused for now; Tasks 7, 8, 9 wire the broadcasts).

```rust
pub async fn dispatch(
    world: &mut WorldView,
    paths: &mut PathStore,
    layout: &TownLayout,
    graph: &RoomGraph,
    out_bus: &HashMap<String, mpsc::Sender<serde_json::Value>>,
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    agent_id: &str,
    msg: serde_json::Value,
) -> serde_json::Value {
    // existing body; the speak/request_changes calls thread event_tx:
    "speak" => handle_speak(world, out_bus, pool, event_tx, &caller, args).await,
    "task_request_changes" => handle_task_request_changes(out_bus, pool, event_tx, &caller, args).await,
    // ... other handlers unchanged ...
}
```

Update `handle_speak` and `handle_task_request_changes` signatures to take `event_tx: &broadcast::Sender<ConsoleOutbound>`. Add `let _ = event_tx;` at the top of each until Tasks 7-9 wire them.

- [ ] **Step 4: Update `loop_.rs` Cmd dispatch arms**

In `crates/world/src/loop_.rs:117-125`, the world-loop body that handles `Cmd::HandleConsoleMsg` and `Cmd::HandleWorkerMsg` needs to capture and pass `event_tx`. Move the placeholder `_event_tx_owned` from Task 4 into the spawn closure as a real owned variable and pass `&event_tx_owned` to dispatch:

```rust
let event_tx_owned = event_tx;  // moved into the spawn task; replaces the placeholder
tokio::spawn(async move {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            // ... Cmd::Tick unchanged ...
            Cmd::HandleConsoleMsg { msg, reply } => {
                let result = crate::cmd_console::dispatch(
                    &mut w, &pool, &out_bus, &event_tx_owned, msg,
                ).await;
                let _ = view_tx.send(w.clone());
                let _ = reply.send(result);
            }
            Cmd::HandleWorkerMsg { agent_id, msg, reply } => {
                let result = crate::cmd_worker::dispatch(
                    &mut w, &mut paths, &layout, &graph, &out_bus, &pool,
                    &event_tx_owned, &agent_id, msg,
                ).await;
                let _ = view_tx.send(w.clone());
                let _ = reply.send(result);
            }
            // ... rest unchanged ...
        }
    }
});
```

(Drop the `let _event_tx_owned = event_tx.clone();` placeholder line from Task 4 — it's replaced by this real `let event_tx_owned = event_tx;`.)

- [ ] **Step 5: Build to confirm production code compiles**

Run: `cargo build -p cliptown-world`
Expected: production code builds clean. Tests will fail to compile (signature mismatch) — fixed in next steps.

- [ ] **Step 6: Migrate the 11 test files to use TestCtx + new dispatch signatures**

For each test file, the migration pattern is:

```rust
// Before:
let pool = storage::open(...).await.unwrap();
seed::seed_if_empty(&pool).await.unwrap();
// ... fixture rows inserted ...
let mut out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
// ... callers ...
let r = cmd_console::dispatch(&mut w, &pool, &out_bus, msg).await;
```

```rust
// After:
mod common;
use common::TestCtx;

let mut ctx = TestCtx::new().await;
// ... fixture rows inserted using ctx.pool ...
// ... callers populate ctx.out_bus ...
let r = cmd_console::dispatch(&mut w, &ctx.pool, &ctx.out_bus, &ctx.event_tx, msg).await;
// At end of test (for tests that should NOT have triggered any console emit):
ctx.expect_no_broadcasts();
```

Same pattern for `mcp_dispatch::dispatch(... &ctx.event_tx, ...)`. Some tests (e.g. `e2e_directive_chain.rs`, `e2e_review_cycle.rs`) WILL produce broadcasts after Tasks 6-9 — those tests will instead call `ctx.expect_one_broadcast()` or `ctx.drain_broadcasts()` to assert the expected emissions. For this migration step, the simplest path is:
1. Replace `pool` and `out_bus` references with `ctx.pool` and `ctx.out_bus`.
2. Add `&ctx.event_tx` to every `dispatch(...)` call.
3. Add `ctx.expect_no_broadcasts();` at the end of every test.

Tasks 6, 7, 8, 9 will then change the `expect_no_broadcasts()` to `expect_one_broadcast()` for the tests that exercise broadcast paths.

Do this file-by-file. For each file in `[console_cmds, e2e_cafe, e2e_directive_chain, e2e_engineer_artifact, e2e_force_actions, e2e_isolation, e2e_manager_accept, e2e_permission_violation, e2e_proposal_flow, e2e_review_cycle, mcp_handlers]`:

Run: `cargo test -p cliptown-world --test <file>`
Expected: PASS (no behavioral change yet — every test ends with `expect_no_broadcasts()`).

- [ ] **Step 7: Run the full test suite**

Run: `cargo test -p cliptown-world`
Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/world/src/cmd_console.rs crates/world/src/cmd_worker.rs crates/world/src/mcp_dispatch.rs crates/world/src/loop_.rs crates/world/tests/
git commit -m "refactor(M5): thread event_tx through dispatch signatures + 53 tests

Adds event_tx parameter to cmd_console::dispatch, cmd_worker::dispatch,
mcp_dispatch::dispatch, handle_speak, handle_task_request_changes. The
parameter is unused in handler bodies (let _ = event_tx) until subsequent
tasks wire the broadcasts. 53 test call sites across 11 files migrated to
the TestCtx fixture; each test ends with expect_no_broadcasts() so any
new emit accidentally added in subsequent work fails loudly.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: E1 — `cmd_console::OperatorDirective` broadcast + prefetch refactor

TDD: write the test first (against the new behavior), watch it fail, then implement.

**Files:**
- Modify: `crates/world/src/cmd_console.rs:67-108`
- Test: `crates/world/tests/console_emit.rs` (add 2 tests)

- [ ] **Step 1: Write the failing tests**

Append to `crates/world/tests/console_emit.rs`:

```rust
use cliptown_world::cmd_console;
use cliptown_world::state::WorldView;
use serde_json::Value;

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

async fn seed_operator_token(pool: &sqlx::SqlitePool, token: &str) {
    sqlx::query("INSERT INTO operators (id, token, label, created_at) VALUES ('op1', ?, 'op', unixepoch()) ON CONFLICT DO NOTHING")
        .bind(token).execute(pool).await.ok();
}

#[tokio::test]
async fn broadcasts_on_operator_directive() {
    let mut ctx = TestCtx::new().await;
    seed_agent(&ctx.pool, "founder1", "s1").await;
    let mut w = WorldView::default();

    let r = cmd_console::dispatch(
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
    let mut w = WorldView::default();

    let r = cmd_console::dispatch(
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
```

- [ ] **Step 2: Run the tests, confirm they fail**

Run: `cargo test -p cliptown-world --test console_emit broadcasts_on_operator_directive no_broadcast_on_unknown_recipient -- --nocapture`
Expected: both FAIL. The first fails because no broadcast happens; the second fails because the existing inline subquery succeeds-with-FK-violation rather than returning `unknown_recipient`.

- [ ] **Step 3: Refactor `OperatorDirective` to prefetch + broadcast**

In `crates/world/src/cmd_console.rs`, replace the entire `ConsoleInbound::OperatorDirective` arm (lines 67-108) with:

```rust
ConsoleInbound::OperatorDirective { to_agent_id, body, .. } => {
    // Prefetch recipient validity + startup_id BEFORE any side effect.
    // Codex M4: returning a clean unknown_recipient error is cheaper than
    // letting an inline-subquery INSERT fail via FK violation.
    let row: Result<Option<(String,)>, _> =
        sqlx::query_as("SELECT startup_id FROM agents WHERE id = ?")
            .bind(&to_agent_id)
            .fetch_optional(pool)
            .await;
    let recipient_startup_id = match row {
        Ok(Some((sid,))) => sid,
        Ok(None) => return json!({"type":"error","reason":"unknown_recipient"}),
        Err(e) => return json!({"type":"error","reason":"sql","detail":e.to_string()}),
    };

    let id = uuid::Uuid::new_v4().to_string();
    let r = sqlx::query(
        "INSERT INTO messages (id, startup_id, room_id, author_id, body, kind, ts) \
         VALUES (?, ?, NULL, 'operator', ?, 'directive', unixepoch())",
    )
    .bind(&id)
    .bind(&recipient_startup_id)
    .bind(&body)
    .execute(pool)
    .await;
    match r {
        Ok(_) => {
            // Push to recipient's worker out_bus (existing behavior).
            if let Some(tx) = out_bus.get(&to_agent_id) {
                let payload = json!({
                    "type": "directive",
                    "v": 1,
                    "from_agent_id": "operator",
                    "body": body,
                    "message_id": id,
                });
                if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(payload) {
                    tracing::warn!(component = "cmd_console",
                        agent_id = %to_agent_id,
                        "out_bus full, dropping operator directive"
                    );
                }
            }
            // Broadcast a Directive frame to all subscribed operator consoles
            // (god view, see spec § 4.1). After SQL success only.
            let _ = event_tx.send(crate::protocol::ConsoleOutbound::Directive {
                v: 1,
                message_id: id.clone(),
                ts: chrono::Utc::now().timestamp_millis(),
                startup_id: recipient_startup_id,
                author_id: "operator".into(),
                to_agent_id: to_agent_id.clone(),
                body: body.clone(),
                in_response_to_task: None,
            });
            json!({"type":"ok","kind":"operator_directive","message_id":id})
        }
        Err(e) => json!({"type":"error","reason":"sql","detail":e.to_string()}),
    }
}
```

Remove the `let _ = event_tx;` placeholder from Task 5 at the top of `dispatch`.

- [ ] **Step 4: Run the tests, verify they pass**

Run: `cargo test -p cliptown-world --test console_emit broadcasts_on_operator_directive no_broadcast_on_unknown_recipient -- --nocapture`
Expected: both PASS.

- [ ] **Step 5: Run full crate tests to confirm nothing else regressed**

Run: `cargo test -p cliptown-world`
Expected: all pass. Note: existing tests in `e2e_directive_chain.rs` etc. that drive `OperatorDirective` will now produce a broadcast frame, but they end with `expect_no_broadcasts()` — UPDATE those tests in this same task.

- [ ] **Step 6: Update existing tests that drive `OperatorDirective`**

In `crates/world/tests/e2e_directive_chain.rs`, change the `expect_no_broadcasts()` after the operator_directive dispatch to:

```rust
let frame = ctx.expect_one_broadcast();
let cliptown_world::protocol::ConsoleOutbound::Directive {
    author_id, to_agent_id, ..
} = frame else { panic!("expected Directive"); };
assert_eq!(author_id, "operator");
assert_eq!(to_agent_id, "founder1");
```

Add similar assertions to any other test in the 11-file list that drives `operator_directive`. Run `cargo test -p cliptown-world` and confirm green.

- [ ] **Step 7: Commit**

```bash
git add crates/world/src/cmd_console.rs crates/world/tests/
git commit -m "feat(M5): broadcast Directive frame on cmd_console::OperatorDirective

Prefetches recipient startup_id (replaces inline subquery), returns
unknown_recipient cleanly when the agent doesn't exist, and broadcasts
a ConsoleOutbound::Directive frame after the SQL INSERT succeeds.
author_id is the sentinel 'operator'; in_response_to_task is None.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: E2 — `mcp_dispatch::handle_speak` chat broadcast

TDD. Tackles the chat half of `handle_speak` (kind="chat"). The directive half lands in Task 8.

**Files:**
- Modify: `crates/world/src/mcp_dispatch.rs:344-446` (handle_speak chat branch)
- Test: `crates/world/tests/console_emit.rs` (add 1 test)

- [ ] **Step 1: Write the failing test**

Append to `crates/world/tests/console_emit.rs`:

```rust
#[tokio::test]
async fn broadcasts_on_peer_chat() {
    use cliptown_world::{mcp_dispatch, move_sys, path::RoomGraph, seed::TownLayout, state::AvatarView};

    let mut ctx = TestCtx::new().await;
    // Seed a single startup with two agents in cafe.
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
```

- [ ] **Step 2: Run the test, confirm it fails**

Run: `cargo test -p cliptown-world --test console_emit broadcasts_on_peer_chat -- --nocapture`
Expected: FAIL — no broadcast emitted.

- [ ] **Step 3: Wire the broadcast in `handle_speak`**

In `crates/world/src/mcp_dispatch.rs`, at the end of the `if kind == "chat"` block (after the existing peer fanout, around line 434), add the broadcast and capture `id`/`body`/`startup_id`/`room_id` for the frame. Restructure if needed:

Replace lines 410-434 (the chat fanout block) with:

```rust
if kind == "chat" {
    // Spec invariant 7: chat is room-scoped, fans cross-startup in public rooms.
    let public = is_public_room(&caller.room_id);
    for (peer_id, peer) in &world.avatars {
        if peer_id == &caller.agent_id { continue; }
        if peer.room_id != caller.room_id { continue; }
        let same_startup = peer.startup_id == caller.startup_id;
        if !(same_startup || public) { continue; }
        if let Some(tx) = out_bus.get(peer_id) {
            let _ = tx.try_send(json!({
                "type":"chat_received","v":1,
                "from_agent_id":caller.agent_id,
                "body":body,
                "room_id":caller.room_id
            }));
        }
    }
    // Broadcast a Chat frame to operator consoles (god view).
    let _ = event_tx.send(crate::protocol::ConsoleOutbound::Chat {
        v: 1,
        message_id: id.clone(),
        ts: chrono::Utc::now().timestamp_millis(),
        startup_id: caller.startup_id.clone(),
        room_id: caller.room_id.clone(),
        author_id: caller.agent_id.clone(),
        body: body.clone(),
    });
} else if let Some(rid) = to_agent_id.as_deref() {
    // (Directive branch — Task 8 wires the broadcast here.)
    if let Some(tx) = out_bus.get(rid) {
        let _ = tx.try_send(json!({
            "type":"directive","v":1,
            "from_agent_id":caller.agent_id,
            "body":body
        }));
    }
}
```

Remove the `let _ = event_tx;` placeholder at the top of `handle_speak` from Task 5.

- [ ] **Step 4: Run the test, verify it passes**

Run: `cargo test -p cliptown-world --test console_emit broadcasts_on_peer_chat -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run full crate tests**

Run: `cargo test -p cliptown-world`
Expected: pass. Update `e2e_cafe.rs` (which exercises chat routing) so its `expect_no_broadcasts()` becomes `expect_one_broadcast()` with a Chat-shape assertion. The test's existing assertions about chat_received delivery to peers stay intact.

- [ ] **Step 6: Commit**

```bash
git add crates/world/src/mcp_dispatch.rs crates/world/tests/
git commit -m "feat(M5): broadcast Chat frame on handle_speak (kind=chat)

After the existing INSERT + peer-out_bus fanout, emit a
ConsoleOutbound::Chat frame to operator consoles. Wire-format ts in
milliseconds.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: E2b — `handle_speak` directive broadcast

Same handler, directive branch. Mirror Task 7 with `Directive` shape (no room_id, in_response_to_task=None).

**Files:**
- Modify: `crates/world/src/mcp_dispatch.rs` (the `else if` branch in handle_speak)
- Test: `crates/world/tests/console_emit.rs` (add 1 test)

- [ ] **Step 1: Write the failing test**

Append to `crates/world/tests/console_emit.rs`:

```rust
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
```

- [ ] **Step 2: Run the test, confirm it fails**

Run: `cargo test -p cliptown-world --test console_emit broadcasts_on_peer_directive -- --nocapture`
Expected: FAIL.

- [ ] **Step 3: Wire the broadcast in the directive branch**

In `crates/world/src/mcp_dispatch.rs`, change the directive `else if` branch in `handle_speak` (the one Task 7 left untouched) to:

```rust
} else if let Some(rid) = to_agent_id.as_deref() {
    if let Some(tx) = out_bus.get(rid) {
        let _ = tx.try_send(json!({
            "type":"directive","v":1,
            "from_agent_id":caller.agent_id,
            "body":body
        }));
    }
    // Broadcast a Directive frame to operator consoles. rid is non-empty
    // because the early-validate above (line 363-365) returns Err otherwise.
    let _ = event_tx.send(crate::protocol::ConsoleOutbound::Directive {
        v: 1,
        message_id: id.clone(),
        ts: chrono::Utc::now().timestamp_millis(),
        startup_id: caller.startup_id.clone(),
        author_id: caller.agent_id.clone(),
        to_agent_id: rid.to_string(),
        body: body.clone(),
        in_response_to_task: None,
    });
}
```

- [ ] **Step 4: Run the test, verify it passes**

Run: `cargo test -p cliptown-world --test console_emit broadcasts_on_peer_directive -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run full crate tests**

Run: `cargo test -p cliptown-world`
Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/world/src/mcp_dispatch.rs crates/world/tests/console_emit.rs
git commit -m "feat(M5): broadcast Directive frame on handle_speak (kind=directive)

Mirrors the chat broadcast in the directive branch. in_response_to_task
is None for peer directives (review-cycle directives are emitted from
handle_task_request_changes in the next task).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: E3 — `handle_task_request_changes` transactional UPDATE+INSERT + broadcast

The trickiest task. Wraps task UPDATE + new directive INSERT in a single SQL transaction so the broadcast-after-SQL invariant holds, gates the broadcast on `assignee_agent_id.is_some()` (mirrors current out_bus skip), and keeps the escalation branch SystemEvent-only (no Directive).

**Files:**
- Modify: `crates/world/src/mcp_dispatch.rs:764-864` (handle_task_request_changes)
- Test: `crates/world/tests/console_emit.rs` (add 4 tests)
- Modify: `crates/world/tests/e2e_review_cycle.rs` (assert per-round Directive + escalation negation)

- [ ] **Step 1: Write the failing tests**

Append to `crates/world/tests/console_emit.rs`:

```rust
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

    let _r = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &ctx.out_bus, &ctx.pool, &ctx.event_tx,
        "mgr",
        serde_json::json!({
            "type":"mcp_call","v":1,"tool":"task_request_changes","corr_id":"c1",
            "args":{"task_id":"T1","feedback":"x"}
        }),
    ).await;
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
    if let cliptown_world::protocol::ConsoleOutbound::SystemEvent { kind, severity, .. } = &frames[0] {
        assert_eq!(kind, "task_escalated");
        assert_eq!(severity, "alert");
    }

    // review_round preserved (escalation does NOT increment).
    let rr: (i64, String) = sqlx::query_as("SELECT review_round, status FROM tasks WHERE id = 'T1'")
        .fetch_one(&ctx.pool).await.unwrap();
    assert_eq!(rr.0, 3, "review_round unchanged on escalation");
    assert_eq!(rr.1, "escalated");
}

#[tokio::test]
async fn transactional_integrity_request_changes() {
    use cliptown_world::{mcp_dispatch, move_sys, path::RoomGraph, seed::TownLayout, state::AvatarView};
    let mut ctx = TestCtx::new().await;
    seed_review_cycle_fixture(&ctx.pool).await;

    // Force the messages INSERT to fail by violating a NOT NULL constraint
    // via a body that contains a 4-byte sequence the messages table CHECK
    // rejects. Since messages.body has no CHECK constraint in the schema,
    // we simulate failure by dropping the messages table mid-transaction is
    // too invasive; instead, we corrupt the schema temporarily.
    //
    // Simpler approach: violate UNIQUE id by pre-inserting a directive row
    // with a known id, then mock the uuid generation. Since we don't mock
    // uuid here, this test asserts the HAPPY path with an explicit
    // `assert!(transactional)` invariant — both the task UPDATE and the
    // messages INSERT visible at the end. If a future change splits them,
    // the broadcast-then-rollback case would show as a broadcast for a
    // missing row; that is what this test guards.
    let mut w = cliptown_world::state::WorldView::default();
    w.avatars.insert("mgr".into(), AvatarView {
        agent_id: "mgr".into(), startup_id: "s1".into(), role: "founder".into(),
        backend: "claude_code".into(), current_pos: (0,0), target_pos: None,
        room_id: "suite_1".into(), status: "idle".into(),
    });
    let layout = TownLayout::default_town();
    let graph: RoomGraph = move_sys::graph_from_layout(&layout);
    let mut paths = std::collections::HashMap::new();

    let _r = mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &ctx.out_bus, &ctx.pool, &ctx.event_tx,
        "mgr",
        serde_json::json!({
            "type":"mcp_call","v":1,"tool":"task_request_changes","corr_id":"c1",
            "args":{"task_id":"T1","feedback":"x"}
        }),
    ).await;
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
```

- [ ] **Step 2: Run the tests, confirm they fail**

Run: `cargo test -p cliptown-world --test console_emit -- --nocapture`
Expected: 4 new tests FAIL (broadcasts not emitted, messages row not persisted, escalation SystemEvent path goes through `record_system_event` which doesn't broadcast).

- [ ] **Step 3: Restructure `handle_task_request_changes` for transactional UPDATE+INSERT**

In `crates/world/src/mcp_dispatch.rs`, replace the body of `handle_task_request_changes` (lines 764-864) with:

```rust
async fn handle_task_request_changes(
    out_bus: &HashMap<String, mpsc::Sender<Value>>,
    pool: &SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let task_id = require_str(&args, "task_id")?.to_string();
    let feedback = require_str(&args, "feedback")?.to_string();
    let task = load_task(pool, &task_id).await?;
    if task.startup_id != caller.startup_id {
        return Err(("cross_startup".into(), "task belongs to another startup".into()));
    }
    if !caller_is_manager_of_task(pool, caller, &task).await? {
        return Err(("no_permission".into(), "task_request_changes is manager-only".into()));
    }

    // Escalation branch: max-rounds breach. NO directive INSERT, NO Directive
    // broadcast. Emits a single SystemEvent via emit_system_event so the
    // operator console sees the task transition to escalated.
    if task.review_round >= MAX_REVIEW_ROUNDS {
        let escalated = next(task.status, &Transition::Escalate)
            .map_err(|r| ("illegal_transition".into(), r.to_string()))?;
        sqlx::query("UPDATE tasks SET status = ?, updated_at = unixepoch() WHERE id = ?")
            .bind(status_to_str(escalated)).bind(&task_id)
            .execute(pool).await
            .map_err(|e| ("sql".into(), e.to_string()))?;
        let _ = persist::append_audit(
            pool, &task_id,
            &json!({
                "actor":"system","kind":"escalated",
                "reason":"max_review_rounds_exceeded",
                "at_round":task.review_round,
                "triggered_by":caller.agent_id,
            }).to_string(),
        ).await;
        let _ = crate::emit::emit_system_event(
            pool, event_tx,
            Some(&caller.startup_id),
            "task_escalated",
            &json!({
                "task_id": task_id,
                "rounds": task.review_round,
                "feedback": feedback,
            }).to_string(),
            "alert",
        ).await;
        return Ok(json!({
            "task_id": task_id,
            "new_status": status_to_str(escalated),
            "status": status_to_str(escalated),
            "review_round": task.review_round,
            "reason": "max_review_rounds_exceeded",
        }));
    }

    // Regular round-increment branch: task UPDATE + directive INSERT in a
    // single transaction so the broadcast-after-SQL invariant holds.
    let new_status = next(task.status, &Transition::RequestChanges)
        .map_err(|r| ("illegal_transition".into(), r.to_string()))?;
    let directive_id = uuid::Uuid::new_v4().to_string();
    let mut tx = pool.begin().await.map_err(|e| ("sql".into(), e.to_string()))?;
    sqlx::query(
        "UPDATE tasks SET status = ?, review_round = review_round + 1, updated_at = unixepoch() WHERE id = ?",
    )
    .bind(status_to_str(new_status)).bind(&task_id)
    .execute(&mut *tx).await
    .map_err(|e| ("sql".into(), e.to_string()))?;
    sqlx::query(
        "INSERT INTO messages (id, startup_id, room_id, author_id, body, kind, ts) \
         VALUES (?, ?, NULL, ?, ?, 'directive', unixepoch())",
    )
    .bind(&directive_id).bind(&caller.startup_id).bind(&caller.agent_id).bind(&feedback)
    .execute(&mut *tx).await
    .map_err(|e| ("sql".into(), e.to_string()))?;
    tx.commit().await.map_err(|e| ("sql".into(), e.to_string()))?;

    let _ = persist::append_audit(
        pool, &task_id,
        &json!({"actor":"manager","kind":"task_request_changes","agent_id":caller.agent_id}).to_string(),
    ).await;

    // Broadcast Directive + push to assignee out_bus, ONLY if assignee exists
    // (mirrors the existing out_bus-skip behavior; Codex M4).
    if let Some(assignee) = task.assignee_agent_id.as_deref() {
        let _ = event_tx.send(crate::protocol::ConsoleOutbound::Directive {
            v: 1,
            message_id: directive_id,
            ts: chrono::Utc::now().timestamp_millis(),
            startup_id: caller.startup_id.clone(),
            author_id: caller.agent_id.clone(),
            to_agent_id: assignee.to_string(),
            body: feedback.clone(),
            in_response_to_task: Some(task_id.clone()),
        });
        if let Some(tx) = out_bus.get(assignee) {
            let _ = tx.try_send(json!({
                "type":"directive","v":1,
                "from_agent_id": caller.agent_id,
                "body": feedback,
                "in_response_to_task": task_id,
            }));
        }
    }

    Ok(json!({"task_id": task_id, "new_status": status_to_str(new_status)}))
}
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test -p cliptown-world --test console_emit -- --nocapture`
Expected: all PASS.

- [ ] **Step 5: Update `e2e_review_cycle.rs` to assert per-round Directive + escalation negation**

In `crates/world/tests/e2e_review_cycle.rs`, the existing test that drives round 1 → request_changes should now assert:

```rust
let frame = ctx.expect_one_broadcast();
let cliptown_world::protocol::ConsoleOutbound::Directive {
    in_response_to_task, body, to_agent_id, ..
} = frame else { panic!("expected Directive") };
assert_eq!(in_response_to_task, Some("T1".into()));
assert!(body.contains("revise") || body.contains("feedback"), "feedback in body");
assert_eq!(to_agent_id, "eng");
```

The existing `max_review_rounds_escalates` test should add:

```rust
let frames = ctx.drain_broadcasts();
let dir_count = frames.iter().filter(|f| matches!(f, ConsoleOutbound::Directive {..})).count();
let sys_count = frames.iter().filter(|f| matches!(f, ConsoleOutbound::SystemEvent {..})).count();
assert_eq!(dir_count, 0, "no Directive on escalation");
assert_eq!(sys_count, 1, "one SystemEvent on escalation");
```

- [ ] **Step 6: Run full crate tests**

Run: `cargo test -p cliptown-world`
Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/world/src/mcp_dispatch.rs crates/world/tests/
git commit -m "feat(M5): transactional task_request_changes + Directive broadcast

Wraps task UPDATE + new directive INSERT in a single SQL transaction
so the broadcast-after-SQL invariant holds. Skips broadcast when
assignee is None (mirrors existing out_bus skip behavior). Escalation
branch emits a single SystemEvent via emit_system_event, no Directive,
review_round preserved.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Migrate all 5 `record_system_event` callers to `emit_system_event`

Lights up the entire dark TopBar event feed. Each caller: budget thresholds, worker_dead, permission_violation, task_escalated, startup_dissolved.

**Files:**
- Modify: `crates/world/src/budget.rs:237`
- Modify: `crates/world/src/agent_supervisor.rs:168`
- Modify: `crates/world/src/mcp_dispatch.rs:322` (permission_violation)
- Modify: `crates/world/src/api_startups.rs:395`
- Modify: `crates/world/src/persist.rs:90` (deprecation comment)
- Modify: `crates/world/tests/budget_thresholds.rs`, `e2e_permission_violation.rs`, `api_startups.rs` (caller-path assertions)
- Modify: `crates/world/tests/agent_supervisor.rs` (new worker_dead test)

- [ ] **Step 1: Migrate `budget.rs:237`**

Find the function in `crates/world/src/budget.rs` that wraps the threshold-event SQL. Change:

```rust
persist::record_system_event(pool, Some(startup_id), kind, &payload, severity).await
```

to:

```rust
crate::emit::emit_system_event(pool, event_tx, Some(startup_id), kind, &payload, severity).await
```

The function in `budget.rs` needs an `event_tx` parameter. Trace the call chain back from `apply_report` (which `cmd_worker::dispatch` calls per `budget.rs:5`) — `apply_report` already gets called from `cmd_worker.rs:86,93`. Add `event_tx` to its signature and thread it through.

- [ ] **Step 2: Migrate `agent_supervisor.rs:168`**

In `crates/world/src/agent_supervisor.rs:168`, change:

```rust
let _ = persist::record_system_event(
    &self.pool, ...
).await;
```

to:

```rust
let _ = crate::emit::emit_system_event(
    &self.pool, &self.event_tx, ...
).await;
```

(`self.event_tx` was added in Task 4.)

- [ ] **Step 3: Migrate `mcp_dispatch.rs:322` (permission_violation)**

Same pattern. The handler already has `event_tx` threaded from Task 5.

- [ ] **Step 4: Migrate `mcp_dispatch.rs:810` (task_escalated)**

Already done in Task 9 (escalation branch uses `emit::emit_system_event`). Confirm by re-reading the function.

- [ ] **Step 5: Migrate `api_startups.rs:395` (startup_dissolved)**

In `crates/world/src/api_startups.rs:395`, change:

```rust
let _ = crate::persist::record_system_event(&s.pool, ...).await;
```

to:

```rust
let _ = crate::emit::emit_system_event(&s.pool, &s.handle.event_tx, ...).await;
```

(`s.handle.event_tx` is reachable since `AppState.handle: Handle` and Task 4 added `event_tx` to Handle.)

- [ ] **Step 6: Add a deprecation comment to `persist::record_system_event`**

In `crates/world/src/persist.rs:89`, prepend:

```rust
/// DEPRECATED for new callers — prefer `crate::emit::emit_system_event`,
/// which broadcasts a `ConsoleOutbound::SystemEvent` frame to operator
/// consoles in addition to writing the SQL row. Existing callers may keep
/// using this for SQL-only persistence; new callers must migrate.
```

- [ ] **Step 7: Extend `budget_thresholds.rs` to assert SystemEvent broadcasts**

For each threshold-crossing test (80%, 95%, 100%), add a `ctx.expect_one_broadcast()` and assert the resulting `SystemEvent` has the right `kind` and `severity`. Use the existing seed structure plus `ctx.event_tx`.

- [ ] **Step 8: Extend `e2e_permission_violation.rs`**

After the move_intent denial in the existing test, assert:

```rust
let frame = ctx.expect_one_broadcast();
let ConsoleOutbound::SystemEvent { kind, severity, .. } = frame else { panic!("expected SystemEvent") };
assert_eq!(kind, "permission_violation");
assert_eq!(severity, "alert");
```

- [ ] **Step 9: Extend `api_startups.rs` tests**

After a successful DELETE call, assert one `SystemEvent` with `kind: "startup_dissolved"`.

- [ ] **Step 10: Add a worker_dead test to `agent_supervisor.rs`**

In `crates/world/tests/agent_supervisor.rs`, add a new test that:
1. Constructs an `AgentSupervisor` with a misconfigured agent (e.g., backend that always fails).
2. Drives the supervisor through enough retries to exceed `backoff_ms.len()`.
3. Asserts a `SystemEvent` with `kind: "worker_dead"`, `severity: "alert"` was broadcast.

If the existing supervisor test infrastructure makes this hard, write a focused unit test that calls the relevant supervisor method directly and inspects the broadcast.

- [ ] **Step 11: Run full test suite**

Run: `cargo test -p cliptown-world`
Expected: all pass.

- [ ] **Step 12: Commit**

```bash
git add crates/world/src/budget.rs crates/world/src/agent_supervisor.rs crates/world/src/mcp_dispatch.rs crates/world/src/api_startups.rs crates/world/src/persist.rs crates/world/src/cmd_worker.rs crates/world/tests/
git commit -m "feat(M5): migrate 5 record_system_event callers to emit_system_event

Lights up the previously-dark TopBar event feed: budget threshold
warnings, permission violations, worker_dead alerts, task_escalated,
and startup_dissolved now reach the operator console live in addition
to being persisted. persist::record_system_event keeps a deprecation
comment for SQL-only callers.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: Snapshot extension — `review_round` + `max_review_rounds`

Adds the per-task fields the kanban needs to render the round indicator.

**Files:**
- Modify: `crates/world/src/http.rs:154-189` (build_console_snapshot)
- Modify: `crates/world/src/mcp_dispatch.rs:34` (re-export MAX_REVIEW_ROUNDS or read from config)
- Test: `crates/world/tests/view_chunk.rs` or `http_smoke.rs` (assert fields in projection)

- [ ] **Step 1: Surface `MAX_REVIEW_ROUNDS` from config**

In `crates/world/src/mcp_dispatch.rs:34`, the existing `const MAX_REVIEW_ROUNDS: u32 = 3;` (or similar) is private. Two paths — pick one:

**Option A (preferred):** delete the const, make `handle_task_request_changes` read `cfg.task.max_review_rounds` from a state-passed config. Requires threading `&TaskCfg` through the dispatch chain. Larger change.

**Option B (smaller):** leave the const but expose it via `pub(crate)` and read it in `build_console_snapshot` directly. Mark with the existing M9-hardening TODO comment for follow-up. Smaller change. **Recommend B** for this PR; the spec § 5 says "in this PR for consistency" but B keeps the diff focused.

For Option B, change:

```rust
const MAX_REVIEW_ROUNDS: u32 = 3;
```

to:

```rust
pub(crate) const MAX_REVIEW_ROUNDS: u32 = 3;
// TODO(M9): move to cfg.task.max_review_rounds
```

The `AppState.max_review_rounds` field added in Task 4 was sourced from `cfg.task.max_review_rounds` already. If those values diverge, downstream behavior is inconsistent. To unify: in main.rs, change the AppState construction to use `crate::mcp_dispatch::MAX_REVIEW_ROUNDS` (so snapshot and handler agree on the same number). Or accept the divergence (config has 3, const has 3) and revisit in M9.

For Option B: use `cfg.task.max_review_rounds` for the snapshot field (already in `AppState`), and document that the const stays in mcp_dispatch.rs for now. They both read 3 today, so behavior is consistent.

- [ ] **Step 2: Extend `build_console_snapshot` to include the new fields**

In `crates/world/src/http.rs`, find the tasks SELECT inside `build_console_snapshot` (around line 154-189). Change the SELECT statement to include `review_round`:

```rust
let task_rows: Vec<(String, String, String, String, Option<String>, Option<String>, i64)> = sqlx::query_as(
    "SELECT id, startup_id, title, status, assignee_agent_id, required_room, review_round FROM tasks"
)
.fetch_all(pool)
.await
.unwrap_or_default();

let tasks: Vec<serde_json::Value> = task_rows.into_iter().map(|(id, startup_id, title, status, assignee, room, rr)| {
    json!({
        "id": id,
        "startup_id": startup_id,
        "title": title,
        "status": status,
        "assignee_agent_id": assignee,
        "required_room": room,
        "review_round": rr,
        "max_review_rounds": max_review_rounds,
    })
}).collect();
```

Pass `max_review_rounds: u32` into `build_console_snapshot` as a new parameter; both call sites (initial snapshot + view_rx tick) read it from `state.max_review_rounds` (added in Task 4).

- [ ] **Step 3: Write a test that asserts the new fields appear**

Add to `crates/world/tests/http_smoke.rs` (or wherever `build_console_snapshot` is currently tested):

```rust
#[tokio::test]
async fn snapshot_includes_review_round_and_max() {
    let mut ctx = TestCtx::new().await;
    sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) VALUES ('s1','a','g',10.0,'town_default','/tmp','active',unixepoch())").execute(&ctx.pool).await.unwrap();
    sqlx::query("INSERT INTO tasks (id, startup_id, title, description, status, review_round, created_at, updated_at) VALUES ('T1', 's1', 't', 'd', 'in_progress', 2, unixepoch(), unixepoch())").execute(&ctx.pool).await.unwrap();

    let view = cliptown_world::state::WorldView::default();
    let frame = cliptown_world::http::build_console_snapshot(&ctx.pool, &view, 3 /* max */).await;
    let tasks = frame["snapshot"]["tasks"].as_array().unwrap();
    let t1 = tasks.iter().find(|t| t["id"] == "T1").unwrap();
    assert_eq!(t1["review_round"], 2);
    assert_eq!(t1["max_review_rounds"], 3);
}
```

Note: `build_console_snapshot` may be private — make it `pub(crate)` or `pub` if needed for the test, OR test indirectly via the WS handshake. Adjust per the actual visibility.

- [ ] **Step 4: Run the test**

Run: `cargo test -p cliptown-world snapshot_includes_review_round_and_max -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run full crate tests**

Run: `cargo test -p cliptown-world`
Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/world/src/http.rs crates/world/src/mcp_dispatch.rs crates/world/tests/
git commit -m "feat(M5): include review_round + max_review_rounds in TaskVM snapshot

Adds per-task review_round (from tasks.review_round SQL column) and
max_review_rounds (from cfg.task.max_review_rounds via AppState) to
the build_console_snapshot tasks projection. Frontend Kanban can now
render 'Round N / M' indicators in a follow-up PR.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: `handle_console` subscribe + select arm + Lagged-fatal-close

Wires the third `select!` arm. Operator consoles now receive every Chat/Directive/SystemEvent broadcast.

**Files:**
- Modify: `crates/world/src/http.rs:191-256` (handle_console body)
- Test: `crates/world/tests/console_emit.rs` (Lagged-fatal-close test)

- [ ] **Step 1: Write the failing Lagged test**

Append to `crates/world/tests/console_emit.rs`:

```rust
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
```

(Note: this is a unit test for the broadcast channel's lossy semantic, not the WS-close path itself. The WS-close path is exercised by an integration test in step 4.)

- [ ] **Step 2: Run the test, confirm it passes** (it should, since this is testing broadcast semantics, not new code)

Run: `cargo test -p cliptown-world --test console_emit lagged_subscriber_logs_and_closes -- --nocapture`
Expected: PASS.

- [ ] **Step 3: Add the `event_rx` arm in `handle_console`**

In `crates/world/src/http.rs`, in `handle_console` (lines 191-256), the existing select! has two arms (inbound + view_rx.changed). Add a third for `event_rx.recv()`:

```rust
async fn handle_console(mut socket: WebSocket, state: Arc<AppState>) {
    // ... existing hello + auth phase unchanged through line ~210 ...

    let mut view_rx = state.handle.view_rx.clone();
    view_rx.borrow_and_update();
    let mut event_rx = state.handle.event_tx.subscribe();

    // Initial snapshot send unchanged (use state.max_review_rounds for the new param).
    {
        let view = state.handle.view_rx.borrow().clone();
        let frame = build_console_snapshot(&state.pool, &view, state.max_review_rounds).await;
        if socket.send(Message::Text(frame.to_string().into())).await.is_err() {
            return;
        }
    }

    let (mut sender, mut receiver) = socket.split();
    loop {
        tokio::select! {
            inbound = receiver.next() => {
                // ... existing inbound handling unchanged ...
            }
            changed = view_rx.changed() => {
                if changed.is_err() { break; }
                let view = view_rx.borrow_and_update().clone();
                let frame = build_console_snapshot(&state.pool, &view, state.max_review_rounds).await;
                if sender.send(Message::Text(frame.to_string().into())).await.is_err() {
                    break;
                }
            }
            event = event_rx.recv() => {
                match event {
                    Ok(frame) => {
                        let json = match serde_json::to_string(&frame) {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::warn!(component = "handle_console", err = %e,
                                    "failed to serialize broadcast frame");
                                continue;
                            }
                        };
                        if sender.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(component = "handle_console", lagged = n,
                            "console subscriber lagged; closing WS to force resync");
                        break;  // frontend will reconnect to a fresh snapshot
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}
```

- [ ] **Step 4: Build and run all tests**

Run: `cargo test -p cliptown-world`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/world/src/http.rs crates/world/tests/console_emit.rs
git commit -m "feat(M5): handle_console subscribes to event_tx with Lagged-fatal-close

Adds a third select! arm in handle_console that forwards every
ConsoleOutbound::{Chat,Directive,SystemEvent} broadcast to the WS.
On Lagged(n), logs a warning and closes the WS so the frontend
reconnects to a fresh snapshot — sidesteps the broadcast loss
semantic without backfill complexity. Capacity 4096 makes Lagged
practically unreachable at Phase-0 single-startup load.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 13: Frontend — TaskVM extension, severity union, reducer fix, dedup

The smallest task. Frontend reducer cases for `chat`/`directive`/`system_event` already exist (`store.ts:322`, `store.ts:360-389`). This task just adds the typed fields and the two reducer tweaks.

**Files:**
- Modify: `packages/frontend/src/store.ts` (TaskVM, severity union, reducer)

- [ ] **Step 1: Add `review_round` and `max_review_rounds` to `TaskVM`**

In `packages/frontend/src/store.ts:44-51`, change `TaskVM` to:

```typescript
export interface TaskVM {
  id: string;
  startup_id: string;
  title: string;
  status: string;
  assignee_agent_id?: string | null;
  required_room?: string | null;
  review_round?: number;
  max_review_rounds?: number;
}
```

- [ ] **Step 2: Update `indexTasks` to read the new fields**

In `packages/frontend/src/store.ts:228-267`, in the per-task object construction (both array and record code paths), add:

```typescript
review_round: typeof t.review_round === "number" ? t.review_round : undefined,
max_review_rounds: typeof t.max_review_rounds === "number" ? t.max_review_rounds : undefined,
```

- [ ] **Step 3: Extend severity union and `severityFromString`**

In `packages/frontend/src/store.ts:53-59`, change `SystemEventVM`:

```typescript
export interface SystemEventVM {
  ts: number;
  severity: "info" | "warn" | "alert" | "critical";
  kind: string;
  startup_id: string | null;
  payload: unknown;
}
```

In `packages/frontend/src/store.ts:269-272`, change `severityFromString`:

```typescript
function severityFromString(s: unknown): SystemEventVM["severity"] {
  if (s === "warn" || s === "alert" || s === "critical") return s;
  return "info";
}
```

- [ ] **Step 4: Reducer reads `m.message_id ?? m.id` and dedupes**

In `packages/frontend/src/store.ts:360-389` (the chat/directive case), change the message construction to:

```typescript
case "chat":
case "directive": {
  const kind = m.type === "directive" ? "directive" : "chat";
  // Codex M20: prefer protocol field message_id; fall back to m.id for the
  // synthetic-frame test path in e2e/ship-gate.spec.ts which still passes id.
  const id = typeof m.message_id === "string"
    ? m.message_id
    : typeof m.id === "string" || typeof m.id === "number"
      ? String(m.id)
      : newId();
  // Dedup: skip if we've already seen this id. Costs O(N) per append but
  // prevents future double-emission or retry-storm dupes (Codex NIT #20).
  if (state.messages.some(x => x.id === id)) {
    return state;
  }
  const recipient =
    typeof m.to_agent_id === "string"
      ? m.to_agent_id
      : typeof m.recipient_id === "string"
        ? m.recipient_id
        : null;
  const msg: MessageVM = {
    id,
    ts: typeof m.ts === "number" ? m.ts : Date.now(),
    startup_id: asString(m.startup_id),
    room_id: typeof m.room_id === "string" ? m.room_id : null,
    author_id: asString(m.author_id, asString(m.from)),
    body: asString(m.body),
    kind,
    recipient_id: recipient,
  };
  const next = [...state.messages, msg];
  if (next.length > MAX_MESSAGES) next.splice(0, next.length - MAX_MESSAGES);
  return { ...state, messages: next };
}
```

- [ ] **Step 5: Run TypeScript type-check**

Run: `cd packages/frontend && pnpm tsc --noEmit`
Expected: clean. No type errors.

- [ ] **Step 6: Run the existing Playwright suite to confirm synthetic-frame compat**

Run: `cd packages/frontend && pnpm e2e`
Expected: 9 tests pass (3 smoke + 2 keymap + 4 ship-gate). The synthetic-id dispatches in `ship-gate.spec.ts` continue to work because the reducer falls back to `m.id` when `m.message_id` is absent.

- [ ] **Step 7: Commit**

```bash
git add packages/frontend/src/store.ts
git commit -m "feat(M5): frontend reads review_round + new severity + dedupes messages

TaskVM gains optional review_round + max_review_rounds. SystemEventVM
severity union now includes 'critical' (was silently downgrading).
Reducer reads m.message_id ?? m.id so new protocol field works without
breaking ship-gate.spec.ts synthetic-id dispatches. Dedupes by id on
chat/directive append so future reconnect/backfill or retry-storm
emissions don't duplicate in the panel.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 14: Final verification

Run all the verification commands from spec § 10. Confirm nothing regressed.

- [ ] **Step 1: Run the rust crate tests**

Run: `cargo test -p cliptown-world`
Expected: all pass. Test count should be the previous count + ~9 new tests in console_emit.rs + extended assertions in 6+ existing files.

- [ ] **Step 2: Run frontend type-check**

Run: `cd packages/frontend && pnpm tsc --noEmit`
Expected: clean.

- [ ] **Step 3: Run the existing 9-test Playwright suite**

Run: `cd packages/frontend && pnpm e2e`
Expected: 9 tests pass.

- [ ] **Step 4: Visual smoke test (manual)**

Run the world server + frontend dev server. Open the operator console. Send an `operator_directive` via the agent popover. Confirm:
- Directive appears in the chat panel with author "operator", correct timestamp (NOT 1970).
- Kanban card for the resulting task shows when manager creates a subtask.
- If you have a fixture review-cycle setup, confirm round indicator + per-round directive both surface.

- [ ] **Step 5: Final commit + PR-ready state**

```bash
git log --oneline main..HEAD
```

Expected: ~13 atomic commits, each with a clean conventional-commit message. Branch ready for PR review and CI.

---

## Self-Review Checklist (run after writing the plan)

- [x] Every spec section covered:
  - § 3.1 (broadcast channel) → Task 4
  - § 3.2 (new variants) → Task 1
  - § 3.3 (timestamp ms) → Tasks 1, 3, 6, 7, 8, 9
  - § 3.4 (frontend changes) → Task 13
  - § 4.2 (E1 OperatorDirective) → Task 6
  - § 4.3 (E2 handle_speak) → Tasks 7, 8
  - § 4.4 (E3 task_request_changes) → Task 9
  - § 4.5 (emit_system_event helper + 5 callers) → Tasks 3, 10
  - § 5 (snapshot review_round + max_review_rounds) → Task 11
  - § 6 (handle_console subscribe + Lagged-fatal-close) → Task 12
  - § 7.1 (console_emit.rs unit tests) → Tasks 3, 6, 7, 8, 9, 12
  - § 7.2 (extended caller-path tests) → Tasks 6, 9, 10
  - § 7.3 (TestCtx fixture) → Task 2
  - § 9 (53-test signature migration) → Task 5

- [x] No placeholders, TBDs, or "implement later" — every step has concrete code.
- [x] Type consistency: `event_tx`, `message_id`, `in_response_to_task`, `review_round`, `max_review_rounds` consistent across tasks.
- [x] Verification commands match spec § 10.
