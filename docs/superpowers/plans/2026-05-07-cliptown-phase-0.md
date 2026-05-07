# cliptown Phase 0 — Walking Skeleton Implementation Plan (v2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Phase 0 vertical slice of cliptown — a WeWork-style coworking town where multiple AI autonomous startups run real work concurrently with a single human operator who watches from a god view and can possess into any town. Ship gate is the 9 invariants in the spec (`docs/superpowers/specs/2026-05-07-cliptown-design.md` §11).

**Architecture:** Three process types — a Rust **World Server** (single binary, single SQLite, single-threaded event loop with mpsc inbox; **all WS/HTTP handlers dispatch to the loop, none mutate state directly**), N **TS Agent Workers** (one per agent, each hosts an in-process MCP proxy that translates CLI tool calls to `/ws/worker` messages with correlation-id round-trips), and a **TS Frontend** SPA (Vite + React + Pixi). Cross-language types are generated from Rust via `ts-rs` into `crates/world/src/protocol/` whose build script writes `.d.ts` consumed by `@cliptown/protocol`.

**Tech Stack:**
- **Rust 1.75+**: `tokio`, `axum`, `sqlx` (SQLite, WAL), `ts-rs`, `tracing` (JSON formatter), `proptest`, `rmcp` (only used inside Worker via Node bindings; world does NOT speak MCP), `serde`, `pathfinding` (A*), `chrono`.
- **Node 20+** with `pnpm`: Vite, React 18, Pixi.js 8, `pino`, `vitest`, Playwright, `tsx` (for running TS without precompile in dev/tests), `@modelcontextprotocol/sdk`.
- **CLIs (Phase 0)**: Claude Code, Codex CLI, opencode. Probed at world boot.

**v1 → v2 changelog (from `/codex review` 2026-05-07, integrated):**
- Protocol crate moved into `crates/world/src/protocol/` with build script that exports `.d.ts` to `packages/protocol/dist/` on `cargo build`. The separate `crates/protocol` is gone.
- All HTTP/WS handlers now dispatch through the world's mpsc inbox; no path mutates SQLite or in-memory state outside the loop. Single-thread invariant is enforced structurally, not by convention.
- Determinism contract (M0.6) lands at the start: clock + randomness + uuid traits with prod and test impls. Every later task uses these — no `Date.now()`, `Math.random()`, `Uuid::new_v4()` in code paths covered by tests.
- New M1 tasks: world-view watch + chunked snapshot, operator input handling on `/ws/console`, movement subsystem (target/current pos, 1-tile-per-tick, `move_intent` handler), task scheduler (`queued → in_progress`), budget enforcement (80/95/100 thresholds + pause-all).
- M2.3 expands to handle the full MCP surface (`task_failed`, `accept_proposal`, `reject_proposal`, `verify`, `ask_peer`, `observe_world`, `read_artifact`, plus the existing tools).
- M2.2 worker MCP proxy uses correlation-id awaits with explicit listener cleanup — no listener leaks across long sessions.
- M3.1 `SpawnOpts` includes `mcp_url` (the worker's MCP socket), and M3.2 wires the real Claude Code MCP config; the contract test in M3.3 runs the fixture **through** the adapter so normalized hooks are actually validated.
- M3.5 (new) world-side worker supervisor — the existing TS-side supervisor (M3.4) handles CLI restarts inside a worker; M3.5 spawns and respawns the worker processes themselves.
- M4 broken into 13 per-component tasks. IBM Plex woff2 self-hosted (no CDN). Adds: system event feed + history modal, first-run gallery, real `+ New Startup` modal with backend selector reflecting `BackendCatalog`, kanban with drag-drop and stuck indicators, chat panel, agent popover, avatar status overlays, keyboard nav, possess transition (now properly inside M4, not after M9).
- M5.1 spawns founder + engineer + designer (per spec §8.2 role set). M5 also adds `DELETE /api/startups/:id` (M5.8) and a permission-violation E2E (M5.9).
- M5.6 implements the full review cycle (`task_request_changes → round++ → directive → re-submit → accept`) instead of accept-only. Also exercises `max_review_rounds=3` escalation.
- M3.3 fixture CLI compiled to JS (TS executed via `tsx` in tests). Build ordering captured at root (`pnpm build` runs before any Rust test that needs `packages/worker/dist`).
- Sandbox path-escape battery (M1.9) covers Windows absolute paths, Unicode/RTL normalization, trailing-dot Windows-style, hard links, and is replayed by both Rust (world-side `task_done` re-validation) and TS (worker-side `pre_tool` hook).
- M9.1 broken into nine sub-tasks, one per ship-gate invariant.
- Per-step commits replaced with **per-task commits** (still TDD: write test, fail, implement, pass, commit).
- Time estimates revised: tasks marked `(human: ~Xh / CC: ~Y min)`. Several "5-minute" tasks were 30+ minute integration projects; estimates now reflect reality.

**How to read this plan:** Tasks are grouped into 11 milestones (M0–M10). Within a milestone, tasks are **sequential**. Across milestones, see §11.5 of the spec for parallelization lanes — once M0 freezes the protocol module, lanes A, B, C, D can branch. Each task lists exact files, then a checkbox-numbered series of steps. Run every test command shown; commit at the end of each task (not each step). The implementing agent's `/ship` runs `cargo test --workspace && pnpm -r test && pnpm test:e2e` and refuses to land until all green.

**Spec source of truth:** `docs/superpowers/specs/2026-05-07-cliptown-design.md` at commit `62f2f0e`. When this plan and the spec disagree, the spec wins — flag the drift and stop.

---

## Milestone 0 — Bootstrap (sequential, single lane)

Goal: a buildable repo, the protocol module emitting `.d.ts` on `cargo build`, the SQLite v1 migration runnable, the determinism traits in place, CI green.

### Task 0.1: Cargo + pnpm workspace skeleton

**Effort:** human ~1.5h / CC ~10 min. **Files:** `Cargo.toml`, `pnpm-workspace.yaml`, `package.json`, `.gitignore`, `crates/world/Cargo.toml`, `crates/world/src/main.rs`, `crates/world/src/lib.rs`, `packages/{frontend,worker,protocol}/package.json`, `packages/adapters/{claude-code,codex,opencode}/package.json`.

- [ ] **Step 1: Workspace roots**

`Cargo.toml`:
```toml
[workspace]
members = ["crates/world"]
resolver = "2"

[workspace.package]
edition = "2021"
rust-version = "1.75"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
ts-rs = { version = "9", features = ["serde-compat"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
anyhow = "1"
chrono = { version = "0.4", default-features = false, features = ["clock", "serde"] }
```

`pnpm-workspace.yaml`:
```yaml
packages:
  - "packages/*"
  - "packages/adapters/*"
```

`package.json`:
```json
{
  "name": "cliptown",
  "private": true,
  "scripts": {
    "build:rust": "cargo build --workspace",
    "build:ts": "pnpm -r --filter '!@cliptown/frontend' build && pnpm --filter @cliptown/frontend build",
    "build": "pnpm build:rust && pnpm build:ts",
    "test:rust": "cargo test --workspace",
    "test:ts": "pnpm -r test",
    "test": "pnpm build:rust && pnpm build:ts && pnpm test:rust && pnpm test:ts",
    "test:e2e": "playwright test",
    "dev": "concurrently -n world,frontend 'cargo run -p cliptown-world' 'pnpm --filter @cliptown/frontend dev'"
  },
  "devDependencies": { "concurrently": "^9", "playwright": "^1" },
  "packageManager": "pnpm@9.0.0"
}
```

The `build` order — Rust first (it generates `.d.ts` via the protocol module's build script), TS second (consumes them) — is non-negotiable. `test` chains build-before-test so cargo tests that spawn `node packages/worker/dist/index.js` find the artifact.

`.gitignore` (append, do not overwrite the existing `.superpowers/`):
```
target/
node_modules/
dist/
workspaces/
*.db
*.db-shm
*.db-wal
.env
.env.local
playwright-report/
test-results/
```

- [ ] **Step 2: World crate skeleton (no separate protocol crate — protocol lives inside world)**

`crates/world/Cargo.toml`:
```toml
[package]
name = "cliptown-world"
version = "0.0.1"
edition.workspace = true
rust-version.workspace = true
build = "build.rs"

[dependencies]
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
anyhow.workspace = true
chrono.workspace = true
ts-rs.workspace = true

[build-dependencies]
ts-rs.workspace = true

[lib]
path = "src/lib.rs"

[[bin]]
name = "cliptown-world"
path = "src/main.rs"
```

`crates/world/src/lib.rs`:
```rust
pub mod protocol;
```

`crates/world/src/protocol/mod.rs`:
```rust
//! Protocol types shared between world (Rust) and worker/frontend (TS via ts-rs).
//! Add new types in submodules with #[derive(ts_rs::TS)] and
//! #[ts(export, export_to = "../../packages/protocol/dist/")].

mod schema_version;
pub use schema_version::SchemaVersion;
```

`crates/world/src/protocol/schema_version.rs`:
```rust
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
pub struct SchemaVersion { pub v: u8 }

impl SchemaVersion { pub const CURRENT: Self = Self { v: 1 }; }
```

`crates/world/src/main.rs`:
```rust
fn main() {
    tracing_subscriber::fmt().json().init();
    tracing::info!(component = "world", event = "boot", "cliptown world starting");
}
```

`crates/world/build.rs`:
```rust
// build.rs — placeholder. ts-rs `#[ts(export, ...)]` derives run on `cargo test`,
// not on `cargo build`, by design. We trigger them here so a clean `cargo build`
// produces the .d.ts files. See M0.3 Step 3 for the explicit invocation.
fn main() {
    println!("cargo:rerun-if-changed=src/protocol");
}
```

> **Note:** `ts-rs` exports happen during `cargo test` runs, not `cargo build`. M0.3 wires an explicit export step into the build pipeline.

- [ ] **Step 3: TS package skeletons**

For `packages/{frontend,worker,protocol}/` and `packages/adapters/{claude-code,codex,opencode}/`, write a minimal `package.json`:

```json
{
  "name": "@cliptown/<short-name>",
  "version": "0.0.1",
  "private": true,
  "type": "module",
  "scripts": { "build": "echo no-op", "test": "echo no-op" }
}
```

Use `frontend`, `worker`, `protocol`, `adapter-claude-code`, `adapter-codex`, `adapter-opencode` for `<short-name>`.

- [ ] **Step 4: Verify both build systems**

```
cargo build --workspace
pnpm install
```

Both must succeed with no errors.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml pnpm-workspace.yaml package.json .gitignore \
  crates/world packages/ pnpm-lock.yaml Cargo.lock
git commit -m "chore(M0.1): scaffold Cargo + pnpm workspaces; protocol lives in world crate"
```

### Task 0.2: SQLite v1 migration + WAL pragma + storage module

**Effort:** human ~1h / CC ~8 min. **Files:** `crates/world/migrations/0001_initial.sql`, `crates/world/Cargo.toml` (sqlx, tempfile), `crates/world/src/storage.rs`, `crates/world/src/lib.rs`, `crates/world/tests/storage_smoke.rs`.

- [ ] **Step 1: Migration SQL — copy spec §4 verbatim, including all CHECK constraints**

`crates/world/migrations/0001_initial.sql`:
```sql
CREATE TABLE towns (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  map_json TEXT NOT NULL
);

CREATE TABLE rooms (
  id TEXT PRIMARY KEY,
  town_id TEXT NOT NULL REFERENCES towns(id),
  name TEXT NOT NULL,
  type TEXT NOT NULL,
  bounds TEXT NOT NULL,
  private_to_startup_id TEXT
);

CREATE TABLE room_doors (
  id TEXT PRIMARY KEY,
  town_id TEXT NOT NULL REFERENCES towns(id),
  room_a TEXT NOT NULL,
  room_b TEXT NOT NULL,
  tile_x INTEGER NOT NULL,
  tile_y INTEGER NOT NULL
);

CREATE TABLE startups (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  goal_text TEXT NOT NULL,
  budget_cap_usd REAL NOT NULL,
  budget_spent_usd REAL NOT NULL DEFAULT 0,
  town_id TEXT NOT NULL REFERENCES towns(id),
  workspace_path TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('active','paused','dissolved')),
  config_overrides TEXT,
  created_at INTEGER NOT NULL
);

CREATE TABLE agents (
  id TEXT PRIMARY KEY,
  startup_id TEXT NOT NULL REFERENCES startups(id),
  name TEXT NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('founder','engineer','designer')),
  backend TEXT NOT NULL CHECK (backend IN ('claude_code','codex','opencode')),
  model_id TEXT NOT NULL,
  position_json TEXT NOT NULL,
  home_room_id TEXT NOT NULL,
  manager_id TEXT REFERENCES agents(id),
  status TEXT NOT NULL CHECK (status IN ('idle','working','walking','talking','offline'))
);

CREATE TABLE tasks (
  id TEXT PRIMARY KEY,
  startup_id TEXT NOT NULL REFERENCES startups(id),
  parent_id TEXT REFERENCES tasks(id),
  title TEXT NOT NULL,
  description TEXT NOT NULL,
  assignee_agent_id TEXT REFERENCES agents(id),
  required_room TEXT,
  status TEXT NOT NULL CHECK (status IN ('proposed','queued','in_progress','awaiting_review','changes_requested','done','failed','escalated')),
  review_round INTEGER NOT NULL DEFAULT 0,
  audit_trail TEXT NOT NULL DEFAULT '[]',
  epistemic_log TEXT NOT NULL DEFAULT '[]',
  artifact_path TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX tasks_by_assignee ON tasks(assignee_agent_id);
CREATE INDEX tasks_by_startup_status ON tasks(startup_id, status);

CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  startup_id TEXT NOT NULL REFERENCES startups(id),
  room_id TEXT,
  author_id TEXT NOT NULL,
  body TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('chat','directive','system')),
  ts INTEGER NOT NULL
);
CREATE INDEX messages_by_startup_ts ON messages(startup_id, ts DESC);

CREATE TABLE budget_events (
  id TEXT PRIMARY KEY,
  startup_id TEXT NOT NULL REFERENCES startups(id),
  agent_id TEXT REFERENCES agents(id),
  task_id TEXT REFERENCES tasks(id),
  in_tokens INTEGER NOT NULL,
  out_tokens INTEGER NOT NULL,
  cost_usd REAL NOT NULL,
  model_id TEXT NOT NULL,
  ts INTEGER NOT NULL
);

CREATE TABLE fs_audit (
  id TEXT PRIMARY KEY,
  startup_id TEXT NOT NULL REFERENCES startups(id),
  agent_id TEXT REFERENCES agents(id),
  op TEXT NOT NULL,
  path TEXT NOT NULL,
  bytes INTEGER NOT NULL DEFAULT 0,
  ok INTEGER NOT NULL,
  error TEXT,
  ts INTEGER NOT NULL
);

CREATE TABLE system_events (
  id TEXT PRIMARY KEY,
  startup_id TEXT,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  severity TEXT NOT NULL CHECK (severity IN ('info','warn','alert','critical')),
  ts INTEGER NOT NULL
);
CREATE INDEX system_events_recent ON system_events(ts DESC);
```

- [ ] **Step 2: deps**

`crates/world/Cargo.toml` `[dependencies]`:
```toml
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "macros", "migrate"] }
uuid = { version = "1", features = ["v4"] }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Storage initializer (mandatory pragmas)**

`crates/world/src/storage.rs`:
```rust
use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

pub async fn open(path: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(path)?
        .create_if_missing(true)
        .pragma("journal_mode", "WAL")
        .pragma("synchronous", "NORMAL")
        .pragma("foreign_keys", "ON")
        .busy_timeout(std::time::Duration::from_millis(5000));
    let pool = SqlitePoolOptions::new().connect_with(opts).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
```

`lib.rs`: `pub mod storage;`

- [ ] **Step 4: Smoke test**

`crates/world/tests/storage_smoke.rs`:
```rust
#[tokio::test]
async fn storage_opens_and_runs_migrations_in_tempdir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let pool = cliptown_world::storage::open(path.to_str().unwrap()).await.unwrap();
    let row: (String,) = sqlx::query_as("PRAGMA journal_mode").fetch_one(&pool).await.unwrap();
    assert_eq!(row.0.to_lowercase(), "wal");
    let row: (i64,) = sqlx::query_as("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='startups'").fetch_one(&pool).await.unwrap();
    assert_eq!(row.0, 1);
}
```

Run: `cargo test -p cliptown-world --test storage_smoke` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/world Cargo.lock
git commit -m "feat(M0.2): SQLite v1 migration + WAL pragma init"
```

### Task 0.3: ts-rs export pipeline + protocol module + first WS message types

**Effort:** human ~2h / CC ~12 min. **Files:** `crates/world/src/protocol/{mod,schema_version,ws_messages}.rs`, `crates/world/build.rs`, `crates/world/Cargo.toml`, `packages/protocol/{package.json,index.d.ts,dist/.gitkeep}`, `crates/world/tests/ts_export.rs`, repo root `pnpm-workspace.yaml` already includes packages/protocol.

- [ ] **Step 1: Define core WS message envelope types**

`crates/world/src/protocol/ws_messages.rs`:
```rust
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Serialize, Deserialize, TS, Clone)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerInbound {
    Hello { v: u8, agent_id: String, startup_id: String, secret: String },
    McpCall { v: u8, corr_id: String, tool: String, args: serde_json::Value },
    ReportBudget { v: u8, in_tokens: u64, out_tokens: u64, model_id: String, task_id: Option<String> },
    ReportFsOp { v: u8, op: String, path: String, bytes: i64, ok: bool, error: Option<String> },
    CliSessionStarted { v: u8, task_id: Option<String>, prompt_hash: String },
    CliSessionEnded { v: u8, task_id: Option<String>, exit_code: i32, summary: Option<String> },
    TaskProgress { v: u8, task_id: String, note: String },
}

#[derive(Debug, Serialize, Deserialize, TS, Clone)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerOutbound {
    McpReply { v: u8, corr_id: String, result: serde_json::Value },
    McpError { v: u8, corr_id: String, code: String, message: String },
    WorldState { v: u8, snapshot: serde_json::Value },
    WorldStateChunk { v: u8, seq: u32, total: u32, payload: serde_json::Value },
    WorldStateEnd { v: u8 },
    TaskAssigned { v: u8, task_id: String, title: String, description: String, required_room: Option<String>, parent_id: Option<String> },
    SubtaskProposed { v: u8, parent_id: String, proposed_task_id: String, proposer_agent_id: String, title: String, description: String, suggested_assignee_role: Option<String> },
    SubtaskDone { v: u8, parent_id: String, child_id: String, artifact_path: String, review_round: u32 },
    Directive { v: u8, from_agent_id: String, body: String, in_response_to_task: Option<String> },
    ProximityTick { v: u8, room_id: String, members: Vec<serde_json::Value> },
    ChatReceived { v: u8, from_agent_id: String, body: String, room_id: String },
    MoveComplete { v: u8, room_id: String },
    MoveFailed { v: u8, reason: String },
    BudgetWarning { v: u8, remaining_usd: f64, percent_used: u32 },
    Pause,
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize, TS, Clone)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConsoleInbound {
    Hello { v: u8, operator_token: String },
    OperatorMove { v: u8, target_x: i32, target_y: i32 },
    OperatorPossess { v: u8, startup_id: String },
    OperatorUnpossess { v: u8 },
    OperatorDirective { v: u8, to_agent_id: String, body: String },
    OperatorAcceptProposal { v: u8, task_id: String, assignee_agent_id: String, required_room: Option<String> },
    OperatorRejectProposal { v: u8, task_id: String, reason: String },
    OperatorForceAccept { v: u8, task_id: String },
    OperatorForceFail { v: u8, task_id: String, note: String },
    OperatorRecheckBackends,
}

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
}
```

`crates/world/src/protocol/mod.rs`:
```rust
mod schema_version;
mod ws_messages;
pub use schema_version::SchemaVersion;
pub use ws_messages::*;
```

- [ ] **Step 2: Wire export trigger into the build pipeline**

`ts-rs` exports run on `cargo test`. To make `pnpm build` produce `.d.ts` deterministically, add a Cargo alias. Create `.cargo/config.toml`:
```toml
[alias]
ts-export = "test -p cliptown-world --lib --features _ts_export -- --ignored ts_rs_export"
```

`crates/world/Cargo.toml` add a feature:
```toml
[features]
_ts_export = []
```

`crates/world/src/lib.rs` (append):
```rust
#[cfg(test)]
mod ts_export {
    /// Forces the ts-rs export side-effect on `cargo test`. Ignored in normal runs;
    /// run via `cargo ts-export` from package.json's `build:rust` script.
    #[test]
    #[ignore]
    fn ts_rs_export() {
        use crate::protocol::*;
        let _ = SchemaVersion::CURRENT;
    }
}
```

Update repo `package.json` `scripts.build:rust`:
```json
"build:rust": "cargo build --workspace && cargo test -p cliptown-world --lib -- --ignored ts_rs_export"
```

- [ ] **Step 3: TS protocol package consumes the generated files**

`packages/protocol/dist/.gitkeep`: empty file.

`packages/protocol/index.d.ts`:
```typescript
// Generated by ts-rs from crates/world/src/protocol/. Do not edit by hand.
// New exports appear in dist/ and must be re-exported here.
export type { SchemaVersion } from "./dist/SchemaVersion";
export type { WorkerInbound } from "./dist/WorkerInbound";
export type { WorkerOutbound } from "./dist/WorkerOutbound";
export type { ConsoleInbound } from "./dist/ConsoleInbound";
export type { ConsoleOutbound } from "./dist/ConsoleOutbound";
```

`packages/protocol/package.json`:
```json
{
  "name": "@cliptown/protocol",
  "version": "0.0.1",
  "private": true,
  "type": "module",
  "main": "index.d.ts",
  "types": "index.d.ts",
  "scripts": {
    "build": "test -f dist/SchemaVersion.ts || (echo 'Run pnpm build:rust first' && exit 1)",
    "test": "echo no-op"
  }
}
```

- [ ] **Step 4: Verify**

```
pnpm build:rust
ls packages/protocol/dist/
```

Expected files: `SchemaVersion.ts`, `WorkerInbound.ts`, `WorkerOutbound.ts`, `ConsoleInbound.ts`, `ConsoleOutbound.ts`.

- [ ] **Step 5: Commit**

```bash
git add crates/world packages/protocol .cargo package.json
git commit -m "feat(M0.3): protocol module + ts-rs export pipeline wired into pnpm build"
```

### Task 0.4: cliptown.toml + config loader

**Effort:** human ~30min / CC ~5 min. **Files:** `cliptown.toml`, `crates/world/Cargo.toml` (toml dep), `crates/world/src/config.rs`, `crates/world/src/lib.rs`, `crates/world/tests/config_smoke.rs`.

- [ ] **Step 1: Default config — copy verbatim from spec §3.5 + kanban additions**

`cliptown.toml`:
```toml
[world]
tick_hz = 1
position_snapshot_every_ticks = 60

[task]
max_review_rounds = 3
max_llm_turns_per_task = 20

[epistemic]
max_hypotheses_per_task = 8
max_tests_per_hypothesis = 5
non_trivial_description_token_threshold = 60

[budget]
warn_pct = 80
no_new_task_pct = 95
pause_all_pct = 100

[supervisor]
worker_respawn_backoff_seconds = [1, 5, 30]
worker_respawn_max_attempts = 3

[possess]
operator_keepalive_timeout_seconds = 30

[kanban]
stuck_warn_minutes = 5
stuck_alert_minutes = 30
```

- [ ] **Step 2: Loader** (same as v1)

`crates/world/Cargo.toml` add `toml = "0.8"`. `src/config.rs` with the structs from spec; `pub mod config;` in `lib.rs`.

- [ ] **Step 3: Smoke** asserting all top-level sections load without panic.

- [ ] **Step 4: Commit**

```bash
git add cliptown.toml crates/world
git commit -m "feat(M0.4): cliptown.toml loader with full Phase 0 surface"
```

### Task 0.5: CI skeleton (with build ordering)

**Effort:** human ~30min / CC ~5 min. **Files:** `.github/workflows/ci.yml`.

- [ ] **Step 1: CI workflow that builds Rust → TS → tests both → e2e (mocked)**

```yaml
name: CI
on: [push, pull_request]
jobs:
  build-and-test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: actions/setup-node@v4
        with: { node-version: 20 }
      - uses: pnpm/action-setup@v4
        with: { version: 9 }
      - run: pnpm install --frozen-lockfile
      - name: Build (Rust → TS)
        run: pnpm build
      - name: Rust tests
        run: pnpm test:rust
      - name: TS tests
        run: pnpm test:ts
      - name: E2E (mocked LLM)
        run: pnpm test:e2e
```

- [ ] **Step 2: Commit**

```bash
git add .github
git commit -m "ci(M0.5): cargo + pnpm + e2e with mandatory build-before-test order"
```

### Task 0.6: Determinism contract — clock, randomness, uuid traits

**Effort:** human ~1h / CC ~7 min. **Files:** `crates/world/src/det.rs`, `crates/world/src/lib.rs`, `crates/world/tests/det_unit.rs`, `packages/worker/src/det.ts`.

> **Why this lands now:** every later task references the determinism contract. Without it, every test that mocks time or randomness has to invent its own pattern.

- [ ] **Step 1: Rust traits + prod/test impls**

`crates/world/src/det.rs`:
```rust
use std::sync::Arc;
use std::time::SystemTime;
use uuid::Uuid;

pub trait Clock: Send + Sync { fn now_unix(&self) -> i64; }
pub trait Randomness: Send + Sync { fn next_u32(&self) -> u32; }
pub trait UuidGen: Send + Sync { fn new(&self) -> String; }

pub struct ProdClock;
impl Clock for ProdClock {
    fn now_unix(&self) -> i64 {
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() as i64
    }
}
pub struct ProdRandom;
impl Randomness for ProdRandom { fn next_u32(&self) -> u32 { rand::random() } }
pub struct ProdUuid;
impl UuidGen for ProdUuid { fn new(&self) -> String { Uuid::new_v4().to_string() } }

#[derive(Clone)]
pub struct DetCtx {
    pub clock: Arc<dyn Clock>,
    pub random: Arc<dyn Randomness>,
    pub uuid: Arc<dyn UuidGen>,
}
impl DetCtx {
    pub fn prod() -> Self { Self { clock: Arc::new(ProdClock), random: Arc::new(ProdRandom), uuid: Arc::new(ProdUuid) } }
}

#[cfg(any(test, feature = "test_det"))]
pub mod testing {
    use super::*;
    use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};
    pub struct FakeClock(AtomicI64);
    impl FakeClock { pub fn at(t: i64) -> Self { Self(AtomicI64::new(t)) } pub fn advance(&self, by: i64) { self.0.fetch_add(by, Ordering::SeqCst); } }
    impl Clock for FakeClock { fn now_unix(&self) -> i64 { self.0.load(Ordering::SeqCst) } }
    pub struct SeededRandom(AtomicU32);
    impl SeededRandom { pub fn seed(s: u32) -> Self { Self(AtomicU32::new(s)) } }
    impl Randomness for SeededRandom {
        fn next_u32(&self) -> u32 {
            // xorshift32
            let mut x = self.0.load(Ordering::SeqCst);
            x ^= x << 13; x ^= x >> 17; x ^= x << 5;
            self.0.store(x, Ordering::SeqCst);
            x
        }
    }
    pub struct SeededUuid(AtomicU64);
    impl SeededUuid { pub fn seed(s: u64) -> Self { Self(AtomicU64::new(s)) } }
    impl UuidGen for SeededUuid {
        fn new(&self) -> String {
            let n = self.0.fetch_add(1, Ordering::SeqCst);
            format!("00000000-0000-0000-0000-{:012x}", n)
        }
    }
    pub fn ctx(t0: i64, seed: u32) -> DetCtx {
        DetCtx {
            clock: Arc::new(FakeClock::at(t0)),
            random: Arc::new(SeededRandom::seed(seed)),
            uuid: Arc::new(SeededUuid::seed(0)),
        }
    }
}
```

Add `rand = "0.8"` to deps. `lib.rs`: `pub mod det;`.

- [ ] **Step 2: TS mirror**

`packages/worker/src/det.ts`:
```ts
export interface Clock { nowUnix(): number; }
export interface Randomness { nextU32(): number; }
export interface UuidGen { new(): string; }
export interface DetCtx { clock: Clock; random: Randomness; uuid: UuidGen; }
export function prodCtx(): DetCtx {
  return {
    clock: { nowUnix: () => Math.floor(Date.now() / 1000) },
    random: { nextU32: () => Math.floor(Math.random() * 0x1_0000_0000) },
    uuid: { new: () => crypto.randomUUID() },
  };
}
export function testCtx(t0: number, seed: number): DetCtx {
  let t = t0; let r = seed >>> 0; let u = 0;
  return {
    clock: { nowUnix: () => t },
    random: { nextU32: () => { r ^= r << 13; r ^= r >>> 17; r ^= r << 5; return r >>> 0; } },
    uuid: { new: () => `00000000-0000-0000-0000-${(u++).toString(16).padStart(12, "0")}` },
  };
}
```

- [ ] **Step 3: Unit tests for both sides**

```rust
// crates/world/tests/det_unit.rs
#[test]
fn fake_clock_is_deterministic() {
    use cliptown_world::det::testing::*;
    let c = FakeClock::at(100);
    assert_eq!(<FakeClock as cliptown_world::det::Clock>::now_unix(&c), 100);
    c.advance(5);
    assert_eq!(<FakeClock as cliptown_world::det::Clock>::now_unix(&c), 105);
}
```

```ts
// packages/worker/test/det.test.ts
import { testCtx } from "../src/det";
import { describe, it, expect } from "vitest";
describe("det", () => {
  it("is reproducible with the same seed", () => {
    const a = testCtx(100, 7); const b = testCtx(100, 7);
    expect(a.random.nextU32()).toBe(b.random.nextU32());
    expect(a.uuid.new()).toBe(b.uuid.new());
  });
});
```

- [ ] **Step 4: Commit**

```bash
git add crates/world packages/worker
git commit -m "feat(M0.6): determinism contract — FakeClock + SeededRandom + SeededUuid in Rust and TS"
```

---

## Milestone 1 — World Server Core (Lane A)

Goal: world boots, all WS/HTTP traffic dispatches to a single mpsc loop (not direct mutation), backend catalog probed, town seeded, sandbox + permissions + state machine + persist + scheduler + budget + movement + view-snapshot all in place.

### Task 1.1: Boot + structured logging + health endpoint

**Effort:** human ~1h / CC ~7 min. **Files:** `crates/world/Cargo.toml` (axum, tower-http), `crates/world/src/http.rs`, `crates/world/src/main.rs`, `crates/world/src/lib.rs`, `crates/world/tests/http_smoke.rs`.

- [ ] **Step 1: deps**

```toml
axum = { version = "0.7", features = ["ws", "json"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["trace"] }
hyper = "1"
```

- [ ] **Step 2: `tracing` JSON formatter with required fields**

`main.rs`:
```rust
use anyhow::Result;
use cliptown_world::{config, http, storage};
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    tracing::info!(component = "world", event = "boot");

    let _cfg = config::load_from("cliptown.toml")?;
    let db_path = std::env::var("CLIPTOWN_DB").unwrap_or_else(|_| "cliptown.db".into());
    let pool = storage::open(&db_path).await?;
    tracing::info!(component = "world", event = "storage_ready", db = %db_path);

    let app = http::router_minimal();
    let addr = std::env::var("CLIPTOWN_ADDR").unwrap_or_else(|_| "127.0.0.1:0".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let bound = listener.local_addr()?;
    tracing::info!(component = "world", event = "listening", addr = %bound);
    axum::serve(listener, app).await?;
    Ok(())
}
```

> **`CLIPTOWN_ADDR` defaults to `127.0.0.1:0`** so tests get an OS-assigned port. Production uses `127.0.0.1:8080`.

- [ ] **Step 3: Required JSON log fields enforced**

Every `tracing::info!`, `warn!`, `error!` call site in this codebase must include `component = "world"` (or `"worker:<agent_id>"` / `"adapter:<id>"` in those crates) and `event = "<snake_case>"`. Add a `clippy.toml` lint allowance not required, but add a CI grep step:

`.github/workflows/ci.yml` add a step:
```yaml
- name: Enforce log shape
  run: |
    ! rg -n 'tracing::(info|warn|error)' crates/ | rg -v 'component\s*=' || (echo "Missing component= field" && exit 1)
```

- [ ] **Step 4: Smoke test on a random port**

`crates/world/src/http.rs`:
```rust
use axum::{routing::get, Router, response::Json};
use serde_json::json;

pub fn router_minimal() -> Router {
    Router::new().route("/health", get(|| async { Json(json!({"ok": true})) }))
}
```

`tests/http_smoke.rs`:
```rust
use axum::body::to_bytes;
use tower::ServiceExt;
#[tokio::test]
async fn health_returns_ok_json() {
    let app = cliptown_world::http::router_minimal();
    let req = axum::http::Request::builder().uri("/health").body(axum::body::Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], br#"{"ok":true}"#);
}
```

Run: `cargo test -p cliptown-world --test http_smoke` → PASS.

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(M1.1): world boot + JSON tracing with enforced component/event fields"
```

### Task 1.2: World state + mpsc loop + tick + watch view

**Effort:** human ~2h / CC ~12 min. **Files:** `crates/world/src/state.rs`, `crates/world/src/loop_.rs`, `crates/world/src/lib.rs`, `crates/world/tests/loop_smoke.rs`.

- [ ] **Step 1: World state (typed, single-source-of-truth)**

`crates/world/src/state.rs`:
```rust
use std::collections::HashMap;
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Default, Clone, Serialize, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
pub struct WorldView {
    pub tick_seq: u64,
    pub backend_catalog: HashMap<String, serde_json::Value>,
    pub avatars: HashMap<String, AvatarView>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
pub struct AvatarView {
    pub agent_id: String,
    pub startup_id: String,
    pub role: String,
    pub backend: String,
    pub current_pos: (i32, i32),
    pub target_pos: Option<(i32, i32)>,
    pub room_id: String,
    pub status: String,
}
```

- [ ] **Step 2: Inbox message types — every world mutation MUST go through here**

`crates/world/src/loop_.rs`:
```rust
use crate::state::WorldView;
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Debug)]
pub enum Cmd {
    Tick,
    HandleConsoleMsg { msg: serde_json::Value, reply: oneshot::Sender<serde_json::Value> },
    HandleWorkerMsg { agent_id: String, msg: serde_json::Value, reply: oneshot::Sender<serde_json::Value> },
    BackendCatalogUpdated(std::collections::HashMap<String, serde_json::Value>),
    Shutdown,
}

#[derive(Clone)]
pub struct Handle {
    pub tx: mpsc::Sender<Cmd>,
    pub view_rx: watch::Receiver<WorldView>,
}

pub fn spawn(initial: WorldView) -> Handle {
    let (tx, mut rx) = mpsc::channel::<Cmd>(1024);
    let (view_tx, view_rx) = watch::channel(initial.clone());
    let mut w = initial;
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::Tick => { w.tick_seq = w.tick_seq.wrapping_add(1); let _ = view_tx.send(w.clone()); }
                Cmd::HandleConsoleMsg { msg: _, reply } => { let _ = reply.send(serde_json::json!({"ok": true})); }
                Cmd::HandleWorkerMsg { agent_id: _, msg: _, reply } => { let _ = reply.send(serde_json::json!({"ok": true})); }
                Cmd::BackendCatalogUpdated(c) => { w.backend_catalog = c; let _ = view_tx.send(w.clone()); }
                Cmd::Shutdown => break,
            }
        }
    });
    let timer_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop { interval.tick().await; if timer_tx.send(Cmd::Tick).await.is_err() { break; } }
    });
    Handle { tx, view_rx }
}
```

> **The single-thread invariant:** the world's `WorldView` and any future SQLite write that depends on `WorldView` are mutated only inside this `match cmd` block. Every test that touches the world reaches it through `Handle::tx`. The HTTP/WS layers in 1.3 + later tasks construct a `Cmd` and send it; they never call `sqlx::query` directly during request handling.

`lib.rs`: `pub mod state; pub mod loop_;`

- [ ] **Step 3: Tick smoke**

`tests/loop_smoke.rs`:
```rust
use cliptown_world::{loop_, state::WorldView};
#[tokio::test(start_paused = true)]
async fn tick_advances_seq() {
    let h = loop_::spawn(WorldView::default());
    let initial = h.view_rx.borrow().tick_seq;
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    tokio::task::yield_now().await;
    assert!(h.view_rx.borrow().tick_seq > initial);
}
```

Run → PASS.

- [ ] **Step 4: Commit**

```bash
git commit -am "feat(M1.2): mpsc inbox + 1Hz tick + watch view (single-thread mutation invariant)"
```

### Task 1.3: WS endpoints with auth + dispatch through inbox

**Effort:** human ~3h / CC ~15 min. **Files:** `crates/world/src/http.rs` (replace `router_minimal` with `router`), `crates/world/src/auth.rs`, `crates/world/tests/ws_auth.rs`.

- [ ] **Step 1: Auth helpers**

`crates/world/src/auth.rs`:
```rust
use anyhow::{anyhow, Result};
use sqlx::SqlitePool;

pub async fn validate_operator_token(_pool: &SqlitePool, token: &str) -> Result<()> {
    // Phase 0: single-operator. Token comes from CLIPTOWN_OPERATOR_TOKEN env.
    let expected = std::env::var("CLIPTOWN_OPERATOR_TOKEN").unwrap_or_else(|_| "dev-token".into());
    if token == expected { Ok(()) } else { Err(anyhow!("invalid_operator_token")) }
}

pub async fn validate_agent_secret(pool: &SqlitePool, agent_id: &str, secret: &str) -> Result<String> {
    // Phase 0: secrets stored in agents.config_overrides JSON or env. Returns startup_id.
    let row: (String, Option<String>) = sqlx::query_as("SELECT startup_id, config_overrides FROM agents WHERE id = ?")
        .bind(agent_id).fetch_optional(pool).await?
        .ok_or_else(|| anyhow!("unknown_agent"))?;
    let expected = std::env::var(format!("CLIPTOWN_AGENT_SECRET_{agent_id}")).unwrap_or_else(|_| "dev-secret".into());
    if secret != expected { return Err(anyhow!("invalid_agent_secret")); }
    Ok(row.0)
}
```

- [ ] **Step 2: New `router` with auth + dispatch via `Handle::tx`**

`crates/world/src/http.rs` — replace previous `router_minimal`:
```rust
use axum::{
    extract::{ws::{WebSocket, WebSocketUpgrade, Message}, State},
    response::{Json, Response, IntoResponse},
    routing::{get, post},
    Router,
    http::StatusCode,
};
use serde_json::json;
use std::sync::Arc;
use sqlx::SqlitePool;
use tokio::sync::oneshot;
use crate::loop_::{Cmd, Handle};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub handle: Handle,
    pub catalog: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, serde_json::Value>>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({"ok": true})) }))
        .route("/api/backend-catalog", get(api_catalog))
        .route("/api/backend-catalog/recheck", post(api_recheck))
        .route("/ws/console", get(ws_console))
        .route("/ws/worker", get(ws_worker))
        .with_state(Arc::new(state))
}

async fn api_catalog(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let m = s.catalog.read().await;
    Json(serde_json::to_value(&*m).unwrap())
}

async fn api_recheck(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let new_cat = crate::backend_catalog::probe_all().await;
    let new_json: std::collections::HashMap<_, _> = new_cat.iter()
        .map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap())).collect();
    *s.catalog.write().await = new_json.clone();
    let _ = s.handle.tx.send(Cmd::BackendCatalogUpdated(new_json.clone())).await;
    Json(serde_json::json!({"ok": true, "entries": new_json}))
}

async fn ws_console(ws: WebSocketUpgrade, State(s): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |sock| handle_console(sock, s))
}
async fn ws_worker(ws: WebSocketUpgrade, State(s): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |sock| handle_worker(sock, s))
}

async fn handle_console(mut socket: WebSocket, state: Arc<AppState>) {
    // First frame must be Hello { operator_token }; on failure, close.
    let Some(Ok(Message::Text(first))) = socket.recv().await else { return; };
    let parsed: serde_json::Value = match serde_json::from_str(&first) { Ok(v) => v, Err(_) => return };
    if parsed.get("type") != Some(&serde_json::Value::String("hello".into())) { return; }
    let token = parsed.get("operator_token").and_then(|v| v.as_str()).unwrap_or("");
    if crate::auth::validate_operator_token(&state.pool, token).await.is_err() {
        let _ = socket.send(Message::Text(r#"{"type":"auth_error"}"#.into())).await;
        return;
    }

    while let Some(Ok(Message::Text(txt))) = socket.recv().await {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&txt) else { continue; };
        let (tx, rx) = oneshot::channel();
        let _ = state.handle.tx.send(Cmd::HandleConsoleMsg { msg, reply: tx }).await;
        if let Ok(reply) = rx.await {
            let _ = socket.send(Message::Text(reply.to_string().into())).await;
        }
    }
}

async fn handle_worker(mut socket: WebSocket, state: Arc<AppState>) {
    let Some(Ok(Message::Text(first))) = socket.recv().await else { return; };
    let parsed: serde_json::Value = match serde_json::from_str(&first) { Ok(v) => v, Err(_) => return };
    if parsed.get("type") != Some(&serde_json::Value::String("hello".into())) { return; }
    let agent_id = parsed.get("agent_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let secret = parsed.get("secret").and_then(|v| v.as_str()).unwrap_or("");
    if crate::auth::validate_agent_secret(&state.pool, &agent_id, secret).await.is_err() {
        let _ = socket.send(Message::Text(r#"{"type":"auth_error"}"#.into())).await;
        return;
    }

    while let Some(Ok(Message::Text(txt))) = socket.recv().await {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&txt) else { continue; };
        let (tx, rx) = oneshot::channel();
        let _ = state.handle.tx.send(Cmd::HandleWorkerMsg { agent_id: agent_id.clone(), msg, reply: tx }).await;
        if let Ok(reply) = rx.await {
            let _ = socket.send(Message::Text(reply.to_string().into())).await;
        }
    }
}
```

> Every WS message becomes a `Cmd` on the inbox. Handlers do not touch state; they wait on a oneshot reply. Backpressure: send is awaited; if the inbox is full, the WS task blocks (slow, not lossy).

`lib.rs`: `pub mod auth; pub mod http;`

- [ ] **Step 3: Auth tests**

`tests/ws_auth.rs`:
```rust
// Cover: missing hello, wrong token, valid token. Use tempdir DB.
```
Implement two cases: bad operator token closes WS with `auth_error`; bad agent secret closes WS with `auth_error`.

- [ ] **Step 4: Commit**

```bash
git commit -am "feat(M1.3): /ws/console and /ws/worker with auth + dispatch through mpsc"
```

### Task 1.4: Backend catalog probe (boot + SIGHUP + 5min timer)

**Effort:** human ~1.5h / CC ~10 min. **Files:** `crates/world/src/backend_catalog.rs`, `crates/world/src/main.rs`, `crates/world/tests/backend_catalog.rs`.

- [ ] **Step 1: Probe (same as v1) + struct with version**

`backend_catalog.rs`:
```rust
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
pub struct BackendInfo {
    pub id: String,
    pub available: bool,
    pub version: Option<String>,
    pub install_hint: Option<String>,
    pub last_checked_ts: i64,
}

pub async fn probe_all() -> HashMap<String, BackendInfo> {
    let mut out = HashMap::new();
    for (id, cmd, hint) in [
        ("claude_code", "claude", "Install: npm i -g @anthropic-ai/claude-code"),
        ("codex", "codex", "Install: npm i -g @openai/codex"),
        ("opencode", "opencode", "Install: see https://opencode.ai"),
    ] { out.insert(id.into(), probe_one(id, cmd, hint).await); }
    out
}

async fn probe_one(id: &str, cmd: &str, hint: &str) -> BackendInfo {
    let now = chrono::Utc::now().timestamp();
    let result = tokio::process::Command::new(cmd)
        .arg("--version").kill_on_drop(true)
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut child) = result {
        if let Ok(Ok(out)) = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait_with_output()).await {
            if out.status.success() {
                let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return BackendInfo { id: id.into(), available: true, version: Some(version), install_hint: None, last_checked_ts: now };
            }
        }
    }
    BackendInfo { id: id.into(), available: false, version: None, install_hint: Some(hint.into()), last_checked_ts: now }
}
```

- [ ] **Step 2: Boot + SIGHUP + 5-minute timer in `main.rs`**

```rust
let catalog = std::sync::Arc::new(tokio::sync::RwLock::new(
    backend_catalog::probe_all().await.into_iter()
        .map(|(k,v)| (k, serde_json::to_value(v).unwrap())).collect()
));
let handle = loop_::spawn(state::WorldView::default());

// 5-min refresh
{
    let cat = catalog.clone(); let h = handle.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
        tick.tick().await;
        loop {
            tick.tick().await;
            let new_cat = backend_catalog::probe_all().await;
            let new_json: std::collections::HashMap<_, _> = new_cat.iter()
                .map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap())).collect();
            *cat.write().await = new_json.clone();
            let _ = h.tx.send(loop_::Cmd::BackendCatalogUpdated(new_json)).await;
        }
    });
}

// SIGHUP → recheck (Unix only)
#[cfg(unix)] {
    use tokio::signal::unix::{signal, SignalKind};
    let cat = catalog.clone(); let h = handle.clone();
    tokio::spawn(async move {
        let mut s = signal(SignalKind::hangup()).unwrap();
        while s.recv().await.is_some() {
            let new_cat = backend_catalog::probe_all().await;
            let new_json: std::collections::HashMap<_, _> = new_cat.iter()
                .map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap())).collect();
            *cat.write().await = new_json.clone();
            let _ = h.tx.send(loop_::Cmd::BackendCatalogUpdated(new_json)).await;
        }
    });
}

let app = http::router(http::AppState { pool: pool.clone(), handle: handle.clone(), catalog });
```

- [ ] **Step 3: Test that probe returns 3 entries (regardless of host)**

(Same as v1.)

- [ ] **Step 4: Commit**

```bash
git commit -am "feat(M1.4): backend catalog with boot probe + SIGHUP + 5-min refresh"
```

### Task 1.5: town_default seed (7 rooms, 4 suite slots, 6 doors)

**Effort:** human ~30 min / CC ~5 min. **Files:** `crates/world/src/seed.rs`, `crates/world/tests/seed_smoke.rs`.

> **Title fix from v1:** the seed creates **7 rooms** (4 suite slots + Lobby + Cafe + Library). v1 said "5 rooms" in the title.

(Implementation identical to v1 Task 1.5 step 1. Test asserts `7 rooms, 6 doors, 1 town`.)

Commit: `feat(M1.5): seed town_default with 4 suite slots + 3 common rooms`.

### Task 1.6: Pathfinding — room graph + tile grid

**Effort:** human ~3h / CC ~15 min. **Files:** `crates/world/src/path.rs`, `crates/world/tests/path_unit.rs`.

- [ ] **Step 1: Room-graph A* (waypoints between rooms)**

(Same as v1 Task 1.6 Step 2.)

- [ ] **Step 2: Tile-grid A* inside each room**

```rust
// Given a room's bounds and a list of obstacles (later: furniture), return tile waypoints.
pub fn tile_path(bounds: (i32, i32, i32, i32), from: (i32, i32), to: (i32, i32)) -> Option<Vec<(i32, i32)>> {
    use pathfinding::prelude::astar;
    let result = astar(
        &from,
        |&(x, y)| {
            let mut nbs = vec![];
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let nx = x + dx; let ny = y + dy;
                if nx >= bounds.0 && nx < bounds.0 + bounds.2 && ny >= bounds.1 && ny < bounds.1 + bounds.3 {
                    nbs.push(((nx, ny), 1u32));
                }
            }
            nbs
        },
        |&(x, y)| ((x - to.0).abs() + (y - to.1).abs()) as u32,
        |&p| p == to,
    );
    result.map(|(path, _)| path)
}
```

- [ ] **Step 3: Compose — given `from_room`, `from_tile`, `to_room`, `to_tile`, return Vec<(room_id, tile)>**

```rust
pub fn full_route(graph: &RoomGraph, room_bounds: &std::collections::HashMap<String, (i32,i32,i32,i32)>, from: (&str, (i32, i32)), to: (&str, (i32, i32))) -> Option<Vec<(String, Vec<(i32, i32)>)>> {
    let waypoints = graph.route(from.0, to.0)?;
    let mut current_room = from.0.to_string();
    let mut current_tile = from.1;
    let mut out = vec![];
    for (next_room, door_tile) in &waypoints {
        let bounds = room_bounds.get(&current_room).copied()?;
        let segment = tile_path(bounds, current_tile, *door_tile)?;
        out.push((current_room.clone(), segment));
        current_room = next_room.clone();
        current_tile = *door_tile;
    }
    let bounds = room_bounds.get(&current_room).copied()?;
    let segment = tile_path(bounds, current_tile, to.1)?;
    out.push((current_room, segment));
    Some(out)
}
```

- [ ] **Step 4: Tests** for both layers + full_route end-to-end across rooms.

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(M1.6): room-graph + tile-grid A* with full_route composition"
```

### Task 1.7: Permission predicates + property tests

**Effort:** human ~1h / CC ~7 min. (Same as v1 Task 1.7. Commit `feat(M1.7): permission predicates + invariant 8 property tests`.)

### Task 1.8: Task state machine

**Effort:** human ~1h / CC ~7 min. (Same as v1 Task 1.8. Commit `feat(M1.8): task state machine with proposed/operator-force transitions`.)

### Task 1.9: Sandbox path-escape battery (full per spec §6.3)

**Effort:** human ~2h / CC ~12 min. **Files:** `crates/world/src/sandbox.rs`, `crates/world/tests/sandbox_attacks.rs`, `packages/worker/src/sandbox.ts`, `packages/worker/test/sandbox_attacks.test.ts`.

- [ ] **Step 1: Rust resolver** (same as v1 with these additional rejection rules)

Add to `resolve`:
```rust
// Reject Windows absolute (drive letter or UNC)
if candidate.len() >= 2 && candidate.chars().nth(1) == Some(':') { return Err(anyhow!("windows absolute")); }
if candidate.starts_with("\\\\") || candidate.starts_with("//") { return Err(anyhow!("UNC path forbidden")); }

// Reject Unicode RTL override + Bidi controls
const FORBIDDEN_CTRL: &[char] = &['\u{202E}', '\u{202D}', '\u{202A}', '\u{202B}', '\u{202C}', '\u{200E}', '\u{200F}'];
if candidate.chars().any(|c| FORBIDDEN_CTRL.contains(&c)) { return Err(anyhow!("bidi control char")); }

// Reject unicode normalization mismatch — require NFC
use unicode_normalization::UnicodeNormalization;
let nfc: String = candidate.nfc().collect();
if nfc != candidate { return Err(anyhow!("non-NFC path")); }

// Reject trailing dot (Windows legacy quirk where "foo." resolves to "foo")
if candidate.ends_with('.') || candidate.ends_with(' ') { return Err(anyhow!("trailing dot/space")); }
```

Add `unicode-normalization = "0.1"` to deps.

- [ ] **Step 2: Test battery** — extend v1 with cases for Windows abs, bidi RTL, non-NFC, trailing dot, trailing space, and ensure hard-link symlink tests work on macOS and Linux.

- [ ] **Step 3: Mirror in TS**

`packages/worker/src/sandbox.ts`:
```ts
import * as path from "node:path";
import * as fs from "node:fs";

export function resolveSandbox(root: string, candidate: string): string {
  if (!candidate) throw new Error("empty path");
  if (candidate.includes("\0")) throw new Error("nul byte");
  if (candidate.length > 4096) throw new Error("too long");
  if (path.isAbsolute(candidate)) throw new Error("absolute forbidden");
  if (/^[A-Za-z]:/.test(candidate) || candidate.startsWith("\\\\") || candidate.startsWith("//")) throw new Error("windows abs");
  if (/[‪-‮‎‏]/.test(candidate)) throw new Error("bidi control");
  if (candidate.normalize("NFC") !== candidate) throw new Error("non-NFC");
  if (candidate.endsWith(".") || candidate.endsWith(" ")) throw new Error("trailing dot/space");
  const joined = path.join(root, candidate);
  const realRoot = fs.realpathSync(root);
  let real: string;
  try { real = fs.realpathSync(joined); }
  catch { const parent = path.dirname(joined); real = path.join(fs.realpathSync(parent), path.basename(joined)); }
  if (!real.startsWith(realRoot + path.sep) && real !== realRoot) throw new Error("escapes root");
  return real;
}
```

Replay the same fixture battery in `test/sandbox_attacks.test.ts`.

- [ ] **Step 4: Commit**

```bash
git commit -am "feat(M1.9): sandbox attack battery — Rust + TS replay (Windows abs, RTL, NFC, trailing dot, hard links)"
```

### Task 1.10: Persist helpers + position snapshots

**Effort:** human ~1.5h / CC ~10 min. (Same as v1 Task 1.10 + add `snapshot_positions(pool, &avatars)` writing `agents.position_json` every 60 ticks.)

Commit: `feat(M1.10): persist helpers (audit_trail, fs_audit, position snapshot)`.

### Task 1.11: World-view watch + chunked snapshot

**Effort:** human ~2h / CC ~12 min. **Files:** `crates/world/src/view.rs`, `crates/world/tests/view_chunk.rs`.

- [ ] **Step 1: Build a per-startup `world_view` payload** filtered by viewer (operator sees all; agent sees own + same-room peers + last-20-msgs).

- [ ] **Step 2: Chunk if >256KB** — split into `WorldStateChunk { seq, total }` + `WorldStateEnd`.

- [ ] **Step 3: Tests** — small case fits one frame; large case (synthetic) chunks correctly; reassembly on the receiver side reproduces original.

Commit: `feat(M1.11): world_view snapshot + chunked transport for large state`.

### Task 1.12: Operator input handling on /ws/console

**Effort:** human ~3h / CC ~15 min. **Files:** `crates/world/src/cmd_console.rs`, `crates/world/tests/console_cmds.rs`.

- [ ] **Step 1: Inside `Cmd::HandleConsoleMsg` arm in `loop_::spawn`**, parse the JSON into `ConsoleInbound` and dispatch:

```rust
use crate::protocol::ConsoleInbound;
match serde_json::from_value::<ConsoleInbound>(msg.clone()) {
    Ok(ConsoleInbound::OperatorMove { target_x, target_y, .. }) => { /* mutate operator avatar target_pos */ }
    Ok(ConsoleInbound::OperatorPossess { startup_id, .. }) => { /* spawn operator avatar in lobby */ }
    Ok(ConsoleInbound::OperatorUnpossess { .. }) => { /* despawn operator */ }
    Ok(ConsoleInbound::OperatorDirective { to_agent_id, body, .. }) => { /* append to messages, push to worker */ }
    Ok(ConsoleInbound::OperatorAcceptProposal { task_id, assignee_agent_id, required_room, .. }) => { /* state machine + persist */ }
    Ok(ConsoleInbound::OperatorRejectProposal { task_id, reason, .. }) => { /* state machine + persist */ }
    Ok(ConsoleInbound::OperatorForceAccept { task_id, .. }) => { /* state machine + audit-tag force_accept */ }
    Ok(ConsoleInbound::OperatorForceFail { task_id, note, .. }) => { /* state machine + persist */ }
    Ok(ConsoleInbound::OperatorRecheckBackends) => { /* fire backend probe */ }
    _ => {}
}
let _ = reply.send(serde_json::json!({"ok": true}));
```

- [ ] **Step 2: For each operator command, write the corresponding `audit_trail` entry with `actor = "operator"`.**

- [ ] **Step 3: Tests** — each command produces the right state transition + audit row.

Commit: `feat(M1.12): operator input handling — directive, possess, kanban-drop, recheck`.

### Task 1.13: Movement subsystem

**Effort:** human ~3h / CC ~15 min. **Files:** `crates/world/src/move_sys.rs`, `crates/world/tests/movement.rs`.

- [ ] **Step 1: Add `target_pos` and `current_pos` to `AvatarView`** (already in M1.2).

- [ ] **Step 2: Inside `Cmd::Tick`**, advance each avatar one tile toward its `target_pos` along its computed path. When a tile is the boundary of its current room and matches a door tile to the next room, transition `room_id`.

- [ ] **Step 3: `move_intent` MCP handler in worker dispatch** (will land in M2.3) calls into `move_sys::start_move(world, agent_id, target_room | target_tile)` which precomputes the path. The path is stored on the avatar (memory-only).

- [ ] **Step 4: On arrival, emit `move_complete { room_id }` to the worker via `Cmd::HandleWorkerMsg` reply or a side-channel out-bus** (add a per-agent mpsc that the WS handler reads).

- [ ] **Step 5: `move_failed { reason }`** when path is impossible or permission is denied (cross-startup suite).

- [ ] **Step 6: Tests** — straight-line move; cross-room move; permission deny; no-path.

Commit: `feat(M1.13): tile-by-tile movement with target_pos/current_pos and move_intent handler`.

### Task 1.14: Task scheduler

**Effort:** human ~2h / CC ~12 min. **Files:** `crates/world/src/scheduler.rs`, `crates/world/tests/scheduler.rs`.

- [ ] **Step 1: On each tick, find tasks where `status = 'queued'` and `assignee_agent_id IS NOT NULL`. If the assignee is `idle`, transition the task to `in_progress` and emit `task_assigned` to the assignee's worker.**

- [ ] **Step 2: If the task has a `required_room` and the agent is not in it, also call `move_sys::start_move` toward the room before the agent's CLI is woken.**

- [ ] **Step 3: Tests** — queued+idle → in_progress; queued+working → wait; required_room triggers move first.

Commit: `feat(M1.14): task scheduler — queued → in_progress with required_room dispatch`.

### Task 1.15: Budget enforcement

**Effort:** human ~2h / CC ~12 min. **Files:** `crates/world/src/budget.rs`, `crates/world/tests/budget_thresholds.rs`.

- [ ] **Step 1: Pricing table** (model_id → $/Mtok in/out) embedded in `budget.rs`.

- [ ] **Step 2: On `Cmd::HandleWorkerMsg(report_budget)`:** compute cost, increment `startups.budget_spent_usd`, append a `budget_events` row, and check thresholds:
  - At 80%: emit `system_event { severity: "warn" }` and `Toast` to console.
  - At 95%: refuse new task assignments (scheduler skip), emit `warn`.
  - At 100%: send `Pause` to all of that startup's workers; emit `alert`.

- [ ] **Step 3: Operator can raise cap via `PATCH /api/startups/:id { budget_cap_usd: N }`** — auto-resume.

- [ ] **Step 4: Tests** — cross 80/95/100 with synthetic budget reports and assert side effects.

Commit: `feat(M1.15): budget enforcement with 80/95/100 thresholds and pause-all`.

---

## Milestone 2 — Worker + MCP proxy (Lane C)

### Task 2.1: Worker WS hello + auth

**Effort:** human ~1.5h / CC ~10 min. (Same shape as v1 M2.1 + sets `AGENT_SECRET` env, sends `hello { agent_id, startup_id, secret }`. Test goes through the auth gate.)

Commit: `feat(M2.1): worker WS connect with hello + auth`.

### Task 2.2: In-process MCP proxy with leak-free correlation

**Effort:** human ~3h / CC ~15 min. **Files:** `packages/worker/src/mcp.ts`, `packages/worker/test/mcp_correlation.test.ts`.

- [ ] **Step 1: Server with all 16 MCP tools (list from spec §6.2)** — wire each `CallTool` to `callOverWS` with proper schema typing per tool.

- [ ] **Step 2: `callOverWS` with cleanup**

```ts
async function callOverWS(ws: WorkerHandle, msg: { corr_id: string } & Record<string, unknown>, timeoutMs = 60_000): Promise<unknown> {
  return new Promise((resolve, reject) => {
    let onMsg: ((m: unknown) => void) | null = null;
    let removed = false;
    const cleanup = () => { if (!removed && onMsg) { ws.offMessage(onMsg); removed = true; } };
    const t = setTimeout(() => { cleanup(); reject(new Error("mcp_call_timeout")); }, timeoutMs);
    onMsg = (m: unknown) => {
      const o = m as { type?: string; corr_id?: string; result?: unknown; code?: string; message?: string };
      if (o?.corr_id !== msg.corr_id) return;
      clearTimeout(t); cleanup();
      if (o.type === "mcp_reply") resolve(o.result);
      else if (o.type === "mcp_error") reject(Object.assign(new Error(o.message || "mcp_error"), { code: o.code }));
    };
    ws.onMessage(onMsg);
    ws.send(msg);
  });
}
```

> Add `offMessage(fn)` to `WorkerHandle`: store listeners in a Set, removable on cleanup. No more per-call accumulation.

- [ ] **Step 3: Tests** — timeout cleans listeners; error response cleans listeners; 100 sequential calls do not leak (assert `ws.listenerCount() === 0` after each settles).

Commit: `feat(M2.2): MCP proxy with correlation IDs and listener cleanup`.

### Task 2.3: World-side MCP routing — full handlers

**Effort:** human ~6h / CC ~30 min. **Files:** `crates/world/src/mcp_dispatch.rs`, `crates/world/tests/mcp_handlers.rs`.

- [ ] **Step 1: Inside `Cmd::HandleWorkerMsg(McpCall)` arm, dispatch by tool name. Each handler:**
  - validates permissions (manager-only tools, same-startup only, etc.) — return `mcp_error` if denied
  - performs the world mutation (transactional in SQLite)
  - emits resulting events back to relevant workers (`SubtaskProposed`, `Directive`, `MoveComplete`, etc.)
  - replies with `McpReply { corr_id, result }`

Tools to implement (all 16 from spec §6.2):
1. `move_intent` → `move_sys::start_move`; reply with arrival ETA tick
2. `speak` → append to `messages` table; if `kind: chat`, broadcast to room peers; if `directive`, validate org-graph + send to recipient
3. `task_done` → re-validate `artifact_path` via `sandbox::resolve` against `workspaces/<startup_id>/`, transition task to `awaiting_review`, emit `subtask_done` to manager
4. `task_failed` → transition task to `failed`, audit
5. `subtask_create` → if caller is manager → `queued`; else → `proposed` + `subtask_proposed` to manager
6. `task_accept` → manager-only, `awaiting_review → done`, propagate `subtask_done` to grand-manager
7. `task_request_changes` → manager-only, `awaiting_review → changes_requested`, `review_round++`, emit `directive` with feedback
8. `accept_proposal` → manager-only, `proposed → queued` with assignment
9. `reject_proposal` → manager-only, `proposed → failed`
10. `hypothesis_state` → append to `tasks.epistemic_log`
11. `test_record` → append to `tasks.epistemic_log`
12. `hypothesis_resolve` → append to `tasks.epistemic_log`
13. `verify` → execute the verification method server-side (read_assert, lint_markdown, lint_typescript via TS sidecar, lint_json); reply with observation
14. `ask_peer` → emit `directive`/`chat` and await a single reply within `timeout_ms`; return `{ response | null }`
15. `observe_world` → read-only world query (peers_in_room, my_position, budget_remaining)
16. `read_artifact` → same-startup gate; reply with file content

- [ ] **Step 2: Each tool gets a unit test** asserting permission, mutation, and event emission.

- [ ] **Step 3: Property test** that no MCP call ever causes a state change visible to a different startup.

Commit: `feat(M2.3): MCP world-side dispatch — all 16 tools with permission gates`.

### Task 2.4: Deterministic LLM mock

**Effort:** human ~1h / CC ~7 min. **Files:** `packages/worker/src/llm_mock.ts`, `packages/worker/test/llm_mock.test.ts`.

- [ ] **Step 1: A keyed mock that returns a canned tool_use sequence given a prompt-hash → fixture-name lookup**.

- [ ] **Step 2: Fixture format**: `fixtures/<name>.jsonl` — one tool_use per line.

- [ ] **Step 3: Default fixture for "engineer writes spec.md"** — emits hypothesis_state, writeFile, verify, test_record, hypothesis_resolve, task_done.

Commit: `feat(M2.4): deterministic LLM mock with fixture replay`.

### Task 2.5: Worker entry point + arg parsing

**Effort:** human ~1h / CC ~7 min. (Same as v1 M5.4 with proper arg parsing using `node:util`.)

Commit: `feat(M2.5): worker entry point with arg parsing`.

---

## Milestone 3 — Claude Code Adapter

### Task 3.1: Adapter abstraction (with mcp_url)

**Effort:** human ~30 min / CC ~5 min. (Same as v1 M3.1 + `SpawnOpts` includes `mcp_socket_path: string` — the path to the worker's MCP UNIX socket.)

Commit: `feat(M3.1): adapter interface — SpawnOpts.mcp_socket_path mandatory`.

### Task 3.2: Claude Code adapter — real MCP wiring + hooks

**Effort:** human ~4h / CC ~20 min. **Files:** `packages/adapters/claude-code/src/index.ts`, `packages/adapters/claude-code/test/hooks.test.ts`.

- [ ] **Step 1: Generate Claude Code config file pointing at the worker's MCP socket**

```ts
import { spawn as nodeSpawn } from "node:child_process";
import { writeFile, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const claudeCodeAdapter: BackendAdapter = {
  id: "claude_code",
  capabilities: { hooks: ["pre_tool", "post_tool", "session_stop", "session_error"], inject_context: true, block_on_stop: true },
  async spawn(opts) {
    const cfgDir = await mkdtemp(join(tmpdir(), "ct-cc-"));
    const mcpJson = JSON.stringify({ mcpServers: { cliptown: { type: "stdio", command: "nc", args: ["-U", opts.mcp_socket_path] } } });
    await writeFile(join(cfgDir, "mcp.json"), mcpJson);
    const allowed = "Read,Edit,Write,Glob,Grep,mcp__cliptown__*";
    const child = nodeSpawn(process.env.CLIPTOWN_FIXTURE_CLI || "claude", [
      "--print", opts.prompt,
      "--allowedTools", allowed,
      "--mcp-config", join(cfgDir, "mcp.json"),
      "--strict-mcp-config",
      // hook script forwarding — Claude Code's settings.json hooks call into the worker's IPC
    ], { cwd: opts.cwd, env: opts.env, stdio: ["pipe", "pipe", "pipe"] });
    // ... handle child events, normalize hooks, etc.
    return { pid: child.pid!, wait: () => new Promise(r => child.on("exit", (code, sig) => r({ exit_code: code ?? -1, signal: sig ?? undefined }))) };
  }
};
```

- [ ] **Step 2: Hooks bridge** — Claude Code reads hook scripts from `settings.json`. Generate a temporary settings file in `cfgDir` whose hook scripts are tiny shell scripts that POST to a localhost worker port. Worker exposes a small HTTP listener for this.

- [ ] **Step 3: Tests** — spawn fixture CLI under the adapter, assert pre_tool/post_tool/session_stop hooks fire and produce normalized events.

Commit: `feat(M3.2): Claude Code adapter with real MCP config + normalized hook bridge`.

### Task 3.3: Adapter contract test through the adapter

**Effort:** human ~2h / CC ~12 min. **Files:** `packages/worker/src/fixture-cli.ts` (compiled to `dist/fixture-cli.js` so Node can run it), `packages/worker/test/contract.test.ts`.

- [ ] **Step 1: Fixture CLI compiled** — add `tsx` as devDep; in tests use `npx tsx packages/worker/src/fixture-cli.ts` OR build to `dist/` first via the root `pnpm build`.

- [ ] **Step 2: Contract test runs the fixture *through* `claudeCodeAdapter.spawn()`** with `CLIPTOWN_FIXTURE_CLI` pointing at the fixture binary.

- [ ] **Step 3: Assert hook events arrive in the worker via the bridge** — pre_tool, post_tool, session_stop, with the right payload shape.

Commit: `feat(M3.3): adapter contract test runs fixture through claude_code adapter`.

### Task 3.4: TS-side worker supervisor (CLI restart)

**Effort:** human ~1.5h / CC ~10 min. (Same as v1 M3.4.)

Commit: `feat(M3.4): worker-side CLI supervisor with backoff`.

### Task 3.5: World-side worker process supervisor

**Effort:** human ~2.5h / CC ~12 min. **Files:** `crates/world/src/agent_supervisor.rs`, `crates/world/tests/agent_supervisor.rs`.

- [ ] **Step 1: After M5.2 creates an agent row, world spawns `node packages/worker/dist/index.js --agent-id ... --startup-id ... --world-url ws://127.0.0.1:<port>/ws/worker` as a child process. Track child handle in a per-agent map keyed by `agent_id`.**

- [ ] **Step 2: Detect WS disconnect (worker side) → if child still alive, do nothing; if child died, respawn with backoff [1, 5, 30] up to 3 attempts. After max, emit `system_event { severity: alert, kind: 'worker_dead' }`.**

- [ ] **Step 3: On startup dissolve, SIGTERM all that startup's workers; 5 s grace then SIGKILL.**

- [ ] **Step 4: Tests** — kill a child process, assert respawn; exhaust attempts, assert alert; dissolve, assert kill.

Commit: `feat(M3.5): world-side worker supervisor with backoff and dissolve cleanup`.

---

## Milestone 4 — Frontend (Lane B)

### Task 4.1: Vite + React + Pixi + IBM Plex local font

**Effort:** human ~2h / CC ~12 min. **Files:** `packages/frontend/{package.json, vite.config.ts, tsconfig.json, index.html, src/main.tsx, src/App.tsx, public/fonts/}`.

- [ ] **Step 1: Same setup as v1 M4.1**, plus:

`public/fonts/` — drop self-hosted woff2:
- `IBMPlexSans-Regular.woff2`, `-Medium.woff2`, `-Bold.woff2`
- `IBMPlexMono-Regular.woff2`, `-Medium.woff2`

`src/styles/fonts.css`:
```css
@font-face { font-family: 'IBM Plex Sans'; src: url('/fonts/IBMPlexSans-Regular.woff2') format('woff2'); font-weight: 400; font-display: swap; }
@font-face { font-family: 'IBM Plex Sans'; src: url('/fonts/IBMPlexSans-Medium.woff2') format('woff2'); font-weight: 500; font-display: swap; }
@font-face { font-family: 'IBM Plex Sans'; src: url('/fonts/IBMPlexSans-Bold.woff2') format('woff2'); font-weight: 700; font-display: swap; }
@font-face { font-family: 'IBM Plex Mono'; src: url('/fonts/IBMPlexMono-Regular.woff2') format('woff2'); font-weight: 400; font-display: swap; }
@font-face { font-family: 'IBM Plex Mono'; src: url('/fonts/IBMPlexMono-Medium.woff2') format('woff2'); font-weight: 500; font-display: swap; }
:root { --bg: #FAFAFA; --fg: #1A1A1A; --fg-secondary: #6B6B6B; --border: #E5E5E5; --raised: #FFFFFF; }
body { margin: 0; font-family: 'IBM Plex Sans', sans-serif; color: var(--fg); background: var(--bg); }
code, .mono { font-family: 'IBM Plex Mono', ui-monospace, monospace; }
```

`index.html` adds `<link rel="stylesheet" href="/src/styles/fonts.css">` via Vite.

- [ ] **Step 2: Routes for /console and /town/:id**.

- [ ] **Step 3: Visual smoke** — `pnpm --filter @cliptown/frontend dev`, observe IBM Plex loads (check Network tab).

Commit: `feat(M4.1): Vite/React/Pixi skeleton with self-hosted IBM Plex woff2`.

### Task 4.2: WS hook with reconnect + world_view_snapshot handler

**Effort:** human ~2h / CC ~12 min. **Files:** `packages/frontend/src/{ws.ts, store.ts, hooks/useWorld.ts}`.

- [ ] **Step 1: Reconnecting WS** (same as v1) plus on connect, send `Hello { operator_token }` from `import.meta.env.VITE_OPERATOR_TOKEN || "dev-token"`.

- [ ] **Step 2: Reducer that handles `WorldViewSnapshot`, `WorldViewDelta`, `SystemEvent`, `BackendCatalog`, `Toast`, `Modal`** — typed via `@cliptown/protocol`.

- [ ] **Step 3: Zustand or simple `useReducer` store** holding `{ avatars, startups, tasks, systemEvents, backendCatalog }`.

Commit: `feat(M4.2): WS hook + reducer for ConsoleOutbound messages`.

### Task 4.3: TopBar with system event feed + Recheck

**Effort:** human ~1.5h / CC ~10 min. **Files:** `packages/frontend/src/console/TopBar.tsx`, history modal.

- [ ] **Step 1: Wordmark + 1-line scrolling event feed (last 1–3 events) + + New Startup button + settings menu with Recheck Backends.**

Commit: `feat(M4.3): TopBar with system event feed + recheck`.

### Task 4.4: Sidebar with recency-sort + FLIP animation + hue accents

**Effort:** human ~2h / CC ~12 min. **Files:** `packages/frontend/src/console/Sidebar.tsx`.

- [ ] **Step 1: Sort startups by `last_event_ts` desc on every store update**, animate position changes with FLIP (use `framer-motion` or hand-rolled).

- [ ] **Step 2: Each row has hue accent left edge (8 hues from spec §3.5 palette).**

- [ ] **Step 3: Empty state — "No startups yet" + arrow → top bar.**

Commit: `feat(M4.4): Sidebar with recency sort + FLIP animation`.

### Task 4.5: MainArea Header (budget bar, counts, Open town CTA)

**Effort:** human ~1h / CC ~7 min. (Header band per spec §3.3.)

Commit: `feat(M4.5): MainArea header band`.

### Task 4.6: MainArea Kanban — columns + cards + drag-drop + stuck indicators

**Effort:** human ~5h / CC ~25 min. **Files:** `packages/frontend/src/console/Kanban.tsx`, `Card.tsx`, `dragdrop.ts`.

- [ ] **Step 1: 5 columns** (proposed/queued/in_progress/awaiting_review/done) + footer drawer (failed).

- [ ] **Step 2: Each card: title + assignee monogram in startup hue + review-round dot + chevron to drop into town.** Stuck-indicator left bar amber/red per `cliptown.toml [kanban]`.

- [ ] **Step 3: Drag-drop allowed transitions only** — call `OperatorAcceptProposal`, `OperatorRejectProposal`, `OperatorForceAccept`, `OperatorForceFail`. Forbidden drops snap back with toast "agent-driven only".

- [ ] **Step 4: Tests** — Playwright drag-drop scenarios for each allowed transition.

Commit: `feat(M4.6): kanban with operator-only manager-bypass drag-drop`.

### Task 4.7: New Startup modal (templated cards + blank + backend selector)

**Effort:** human ~3h / CC ~15 min. **Files:** `packages/frontend/src/console/NewStartupModal.tsx`.

- [ ] **Step 1: 3-4 templated cards + Start blank**.

- [ ] **Step 2: Backend selector per role (founder/engineer/designer)** — radio of available backends from `BackendCatalog`. Disabled options strikethrough + install hint.

- [ ] **Step 3: Submit POSTs to `/api/startups`** then redirects to `/town/:id`.

Commit: `feat(M4.7): New Startup modal with backend selector reflecting catalog`.

### Task 4.8: /town TopBar + back + Possess toggle + budget

**Effort:** human ~1h / CC ~7 min. (Same as v1 M4.4 with Possess toggle wired to send `OperatorPossess`/`OperatorUnpossess`.)

Commit: `feat(M4.8): /town top bar with possess toggle`.

### Task 4.9: /town Pixi canvas — rooms + avatars + interpolation

**Effort:** human ~6h / CC ~30 min. **Files:** `packages/frontend/src/town/PixiStage.tsx`, `Avatars.tsx`.

- [ ] **Step 1: Renders 7 rooms with their type colors, doors as gaps in walls.**

- [ ] **Step 2: Avatars** — colored circle, 1-letter monogram, ring style per backend. Status overlays (yellow `…`, red `!`, orange `⏸`, green check).

- [ ] **Step 3: Linear interpolation between snapshots** — server publishes `tick_seq`, frontend interpolates `current_pos → target_pos` over 1s tick interval at 60fps.

- [ ] **Step 4: Click avatar → opens popover (M4.11).**

Commit: `feat(M4.9): Pixi canvas with avatars + tick interpolation`.

### Task 4.10: Chat panel — collapsible, room-scoped, cross-startup tag

**Effort:** human ~2h / CC ~12 min.

- [ ] **Step 1: Bottom-right floating, collapsible to chip with unread count.**

- [ ] **Step 2: Filter by room of selected agent or operator's room.** Cross-startup messages tagged with speaker's startup hue.

- [ ] **Step 3: Send box** when operator possessing — sends `OperatorDirective` or `chat` over `/ws/console`.

Commit: `feat(M4.10): floating chat panel with room scope`.

### Task 4.11: Agent popover — name, role, backend, status, task, directive input

**Effort:** human ~2h / CC ~12 min.

(Per spec §3.3 Agent popover spec.) Commit: `feat(M4.11): agent popover with directive input`.

### Task 4.12: Possess transition — camera ease + avatar fade

**Effort:** human ~3h / CC ~15 min. **Files:** `packages/frontend/src/town/possess.ts`.

(Same as v1 M4.5 — but properly placed before M5.) Commit: `feat(M4.12): possess transition camera ease + avatar fade-in`.

### Task 4.13: Keyboard nav

**Effort:** human ~2h / CC ~12 min. **Files:** `packages/frontend/src/keymap.ts`.

- [ ] **Step 1: Implement spec §3.5 keymap** (j/k/Enter/Esc/t/p/c/g+c/slash) with Tab-order across both routes.

- [ ] **Step 2: Visible focus rings.**

Commit: `feat(M4.13): keyboard navigation per spec §3.5`.

---

## Milestone 5 — End-to-End First Task (Lane E begins)

### Task 5.1: POST /api/startups — founder + engineer + designer

**Effort:** human ~2h / CC ~12 min. **Files:** `crates/world/src/api_startups.rs`, tests.

- [ ] **Step 1: Request schema** — name, goal_text, budget_cap_usd, backends: `{ founder, engineer, designer }`.

- [ ] **Step 2: Transaction** — claim free suite, insert startup, insert 3 agent rows (founder + engineer + designer; founder is manager_id of the other two), set `home_room_id` to claimed suite, position at home desks. Generate per-agent secrets and persist (in env-mapped form for Phase 0).

- [ ] **Step 3: After commit, mkdir `workspaces/<id>/artifacts/`.**

- [ ] **Step 4: Trigger M3.5 supervisor to spawn 3 workers.**

- [ ] **Step 5: Tests** — POST returns 200; DB has 1 startup + 3 agents; 3 workers connected within 5s.

Commit: `feat(M5.1): startup creation with founder + engineer + designer triple`.

### Task 5.2: Suite slot exhaustion + dissolve

**Effort:** human ~1.5h / CC ~10 min. (Already covered M5.1; here add the 5th create returns 409, the DELETE handler in 5.8 frees a slot.)

Commit: `feat(M5.2): suite slot exhaustion guard with 409 response`.

### Task 5.3: Dispatcher: directive → subtask → assigned

**Effort:** human ~2h / CC ~12 min.

- [ ] **Step 1: Operator sends `OperatorDirective { to: founder }` via `/ws/console`.**
- [ ] **Step 2: Founder's CLI sees the directive in next session and emits `subtask_create` MCP.**
- [ ] **Step 3: Scheduler picks up the queued task, transitions to in_progress, emits `task_assigned` to engineer.**

Commit: `feat(M5.3): operator directive → founder subtask → engineer assignment`.

### Task 5.4: Engineer writes artifact (fixture)

**Effort:** human ~2h / CC ~12 min.

- [ ] **Step 1: Engineer fixture executes the fixture from M2.4** — emits hypothesis_state, writeFile to `artifacts/T<id>.md`, verify (read_assert), test_record pass, hypothesis_resolve verified, task_done.

- [ ] **Step 2: World validates artifact path is exactly `workspaces/<startup_id>/artifacts/<task_id>.md`** (re-runs `sandbox::resolve` AND checks the path matches the canonical pattern).

Commit: `feat(M5.4): engineer fixture writes artifact with epistemic discipline`.

### Task 5.5: Manager review — accept

**Effort:** human ~1h / CC ~7 min. Founder fixture receives `subtask_done`, calls `read_artifact`, calls `task_accept`. World transitions to `done`.

Commit: `feat(M5.5): manager accept loop closes invariants 2–5`.

### Task 5.6: Manager review — request_changes round 2

**Effort:** human ~2h / CC ~12 min. **Files:** `crates/world/tests/e2e_review_cycle.rs`.

- [ ] **Step 1: Founder fixture receives subtask_done and conditionally calls `task_request_changes` (round 1) → world transitions, increments `review_round`, emits directive with feedback.**

- [ ] **Step 2: Engineer fixture sees the directive on next session, revises artifact, emits new task_done.**

- [ ] **Step 3: Founder accepts in round 2.**

- [ ] **Step 4: Test asserts** `review_round = 1`, `audit_trail` contains both rounds, final status `done`.

- [ ] **Step 5: Separately test `max_review_rounds = 3`** — fixture configured to always request_changes, after round 3 the world auto-escalates to `escalated`.

Commit: `feat(M5.6): full review cycle E2E + max_review_rounds escalation`.

### Task 5.7: Permission violation E2E

**Effort:** human ~1h / CC ~7 min.

- [ ] **Step 1: α-engineer fixture calls `move_intent { target_room: 'suite_2' }` while β owns suite_2.**
- [ ] **Step 2: World replies `mcp_error { code: "no_permission" }` and writes a `system_events { severity: 'alert' }` row.**

Commit: `feat(M5.7): permission violation rejected with alert event`.

### Task 5.8: DELETE /api/startups/:id — dissolve

**Effort:** human ~2h / CC ~12 min. **Files:** `crates/world/src/api_startups.rs`, tests.

- [ ] **Step 1: Transaction** — mark startup `dissolved`, free its suite (set `private_to_startup_id = NULL`), kill all its workers via M3.5, leave audit history intact.

- [ ] **Step 2: Test** — dissolve frees the slot; new POST succeeds.

Commit: `feat(M5.8): DELETE /api/startups/:id with worker shutdown and slot release`.

---

## Milestone 6 — Multi-tenancy + Isolation (invariant 8)

### Task 6.1: Two startups concurrent — isolation invariant

**Effort:** human ~1.5h / CC ~10 min. (Same as v1 M6.1.)

Commit: `test(M6.1): invariant 8 — directive isolation across startups`.

### Task 6.2: Operator drag-drop accept_proposal flow E2E

**Effort:** human ~2h / CC ~12 min.

- [ ] **Step 1: Engineer fixture (non-manager) calls `subtask_create` → world creates `proposed`, emits `subtask_proposed` to founder + `WorldViewDelta` with the new card.**
- [ ] **Step 2: Operator drags from proposed to queued in /console (Playwright simulates drag). Frontend sends `OperatorAcceptProposal`.**
- [ ] **Step 3: World transitions to queued, scheduler picks up, etc.**
- [ ] **Step 4: Audit row carries `actor=operator`.**

Commit: `feat(M6.2): operator drag-drop proposal acceptance E2E`.

### Task 6.3: Operator force_accept and force_fail

**Effort:** human ~1.5h / CC ~10 min. Drag from awaiting_review → done; drag → failed with required note.

Commit: `feat(M6.3): operator force_accept and force_fail kanban actions`.

---

## Milestone 7 — Cross-startup proximity (invariant 7)

### Task 7.1: Proximity tick — group avatars by room

**Effort:** human ~1h / CC ~7 min. (Same as v1 M7.1, plus the worker out-bus from M1.13 broadcasts `proximity_tick` to each member's worker.)

Commit: `feat(M7.1): proximity_tick groups + emits to workers`.

### Task 7.2: Cross-startup chat in cafe — invariant 7

**Effort:** human ~2h / CC ~12 min. **Files:** `crates/world/tests/e2e_cafe.rs`.

- [ ] **Step 1: α-engineer and β-designer fixtures both emit `move_intent { target_room: cafe }`.**
- [ ] **Step 2: After both arrive, α-engineer's fixture emits `speak { kind: chat, body: "anyone tried mdast?" }`.**
- [ ] **Step 3: Assert β-designer's worker receives `chat_received` with α's body.**

Commit: `test(M7.2): invariant 7 — cross-startup cafe chat delivered`.

---

## Milestone 8 — Codex + opencode adapters (invariant 9)

### Task 8.1: Codex adapter

**Effort:** human ~3h / CC ~15 min. (Same shape as M3.2 with Codex CLI specifics; capability `block_on_stop: false`.)

Commit: `feat(M8.1): Codex CLI adapter with best-effort hook bridge`.

### Task 8.2: opencode adapter

**Effort:** human ~3h / CC ~15 min. (Provider routing, `inject_context: true` via session resume.)

Commit: `feat(M8.2): opencode adapter with provider routing`.

### Task 8.3: Adapter contract test runs across all 3

**Effort:** human ~1h / CC ~7 min. Extend M3.3 to loop over `[claudeCodeAdapter, codexAdapter, opencodeAdapter]`, asserting normalized hooks for each.

Commit: `test(M8.3): adapter contract test passes for all 3 adapters`.

---

## Milestone 9 — Ship gate (Playwright)

### Task 9.1: inv-1 — multiple startups, all 3 adapters connected

**Effort:** human ~2h / CC ~12 min. **Files:** `e2e/inv-1-spawn.spec.ts`.

- [ ] **Step 1: Playwright drives Console UI, creates 3 startups (α/β/γ) with different default backends per role mix.**
- [ ] **Step 2: Asserts 9 workers (3 startups × 3 agents) connected within 30s.**
- [ ] **Step 3: Asserts each backend type has at least one CLI session started (from `cli_session_started` audit).**

Commit: `test(M9.1): invariant 1 — 3 startups + 9 workers + all 3 adapters`.

### Task 9.2: inv-2 — directive → subtask_create → task_assigned

**Effort:** human ~1h / CC ~7 min.

Commit: `test(M9.2): invariant 2`.

### Task 9.3: inv-3 — required_room walk

**Effort:** human ~1.5h / CC ~10 min. Asserts engineer avatar moves through Lobby tiles to Library, A* respects doors.

Commit: `test(M9.3): invariant 3`.

### Task 9.4: inv-4 — artifact at exact path

**Effort:** human ~1h / CC ~7 min.

Commit: `test(M9.4): invariant 4`.

### Task 9.5: inv-5 — epistemic_log has verified hypothesis

**Effort:** human ~1h / CC ~7 min.

Commit: `test(M9.5): invariant 5`.

### Task 9.6: inv-6 — review cycle

**Effort:** human ~1h / CC ~7 min. Reuses M5.6 logic via Playwright UI assertions.

Commit: `test(M9.6): invariant 6`.

### Task 9.7: inv-7 — cross-startup cafe chat

**Effort:** human ~1h / CC ~7 min.

Commit: `test(M9.7): invariant 7`.

### Task 9.8: inv-8 — multi-tenant isolation

**Effort:** human ~1.5h / CC ~10 min.

Commit: `test(M9.8): invariant 8`.

### Task 9.9: inv-9 — all 3 adapters complete a task

**Effort:** human ~1h / CC ~7 min.

Commit: `test(M9.9): invariant 9 — three adapters each ship a task`.

### Task 9.10: Real-LLM opt-in job + budget cap

**Effort:** human ~2h / CC ~12 min. **Files:** `.github/workflows/e2e-real-llm.yml`.

- [ ] **Step 1: Workflow gated on `workflow_dispatch` only.**
- [ ] **Step 2: Sets `E2E_LLM=real`, exports per-run budget cap of $0.50; if breach, fail the run.**

Commit: `ci(M9.10): real-LLM opt-in E2E with budget cap`.

---

## Milestone 10 — Benchmarks + docs

### Task 10.1: Benchmarks + baseline + 20% regression gate

**Effort:** human ~3h / CC ~15 min. **Files:** `packages/frontend/bench/`, `crates/world/benches/`.

- [ ] **Step 1: Frontend bench** — record FCP for /console (target ≤300 ms) and /town/:id (≤500 ms).
- [ ] **Step 2: World bench** — tick latency, mpsc throughput, SQLite write rate.
- [ ] **Step 3: `bench/baselines.json` checked in.** CI runs bench on every PR; fail if any metric regresses >20%.

Commit: `test(M10.1): performance baselines + 20% regression gate`.

### Task 10.2: README + CONTRIBUTING

**Effort:** human ~1.5h / CC ~10 min. (Same as v1 M9.3.)

Commit: `docs(M10.2): README + CONTRIBUTING`.

---

## Self-Review (v2)

Re-checked spec coverage against the v2 plan with the codex findings as a checklist:

| Codex finding | Resolution in v2 |
|---|---|
| Single-thread mpsc invariant violated | M1.2 + M1.3 — every WS message becomes a `Cmd` on the inbox; handlers do not touch state. Auth probe runs in handler before dispatch. |
| TS execution (`node src/fixture-cli.ts`) | M3.3 — fixture compiled to `dist/` via `pnpm build` order; `tsx` for dev runs. |
| Build ordering missing | M0.5 + root `package.json` `test` chains build before tests. |
| Cross-language schema codegen missing | M0.3 wires `cargo test --ignored ts_rs_export` into `pnpm build:rust`. |
| Designer role missing | M5.1 spawns founder + engineer + designer. |
| Possess transition placement | M4.12, before any M5/E2E task. |
| Operator input handling | M1.12 dedicated task. |
| DELETE /api/startups/:id | M5.8. |
| Movement subsystem | M1.13 dedicated task; M2.3 dispatches `move_intent`. |
| Task scheduler | M1.14 dedicated task. |
| Budget enforcement | M1.15 dedicated task. |
| MCP world handlers full set | M2.3 implements all 16 tools. |
| Frontend depth | M4.1–M4.13 broken into 13 per-component tasks. |
| Review cycle invariant 6 untested | M5.6 explicitly tests round++ + escalation. |
| Sandbox battery additions | M1.9 adds Windows abs, RTL, NFC, trailing-dot, and TS replay. |
| Adapter contract test fake | M3.3 runs through the adapter; M8.3 loops over all 3. |
| Worker MCP listener leak | M2.2 with `offMessage` cleanup + sequential 100-call test. |
| Determinism contract missing | M0.6 lands first; later tasks reference it. |
| Logging required fields | M1.1 + CI grep step enforces. |
| Operator session-token + agent secret auth | M1.3 with M0.6 deterministic UUIDs and per-agent env-mapped secrets. |
| World view snapshot + chunk | M1.11 dedicated task. |
| BackendCatalog SIGHUP/timer/Recheck | M1.4 boot + 5-min timer + SIGHUP + POST recheck endpoint + UI menu in M4.3. |
| Per-step commit overhead | Removed; v2 commits per task at the end. |
| Time estimates | Every task carries `human / CC` scales reflecting reality. |
| Task numbering for ship gate | M9.1–M9.9 (one per invariant) + M9.10 (real-LLM) + M10. |

**Spec coverage gaps remaining:** none identified.

**Placeholder scan:** v2 still has `// pseudo` comments in M5.3 dispatcher and M2.3 handlers' permission/audit emission. These are intentional skeletons; the real implementations are several hundred lines each and must follow the spec's typed enums (already defined in M0.3 protocol module).

**Type consistency check:** `WorldView`, `AvatarView`, `BackendInfo`, `WorkerInbound`/`Outbound`, `ConsoleInbound`/`Outbound`, `BackendAdapter`, `SpawnOpts`, `HookHandlers`, `SessionHandle`, `TaskStatus`, `Transition`, `AppState`, `Handle`, `Cmd`, `DetCtx` — all defined in M0–M2 and used consistently in later tasks. No naming drift detected.

---

## Execution Handoff

Plan v2 saved to `docs/superpowers/plans/2026-05-07-cliptown-phase-0.md` (replaces v1; v1 preserved in commit `fbe5b9f`). Two execution options:

**1. Subagent-Driven (recommended)** — fresh subagent per task, two-stage review between tasks. After M0 freezes the protocol module + determinism contract, lanes A (M1), C (M2), B (M4) can run in parallel worktrees. Lane D (M3, M8) joins after M2 contract is set. Lane E (M5–M9) integrates everything.

**2. Inline Execution** — execute tasks in this session using executing-plans, batch checkpoints at milestone boundaries.

Which approach?
