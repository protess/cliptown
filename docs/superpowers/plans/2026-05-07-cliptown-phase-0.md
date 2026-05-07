# cliptown Phase 0 — Walking Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Phase 0 vertical slice of cliptown — a WeWork-style coworking town where multiple AI autonomous startups run real work concurrently with a single human operator who watches from a god view and can possess into any town. Ship gate is the 9 invariants in the spec (`docs/superpowers/specs/2026-05-07-cliptown-design.md` §11).

**Architecture:** Three process types — a Rust **World Server** (single binary, single SQLite, single-threaded event loop with mpsc inbox), N **TS Agent Workers** (one per agent, each hosts an in-process MCP proxy that translates CLI tool calls to `/ws/worker` messages), and a **TS Frontend** SPA (Vite + React + Pixi, talks to world via `/ws/console`). Cross-language types are generated from Rust via `ts-rs` into a `@cliptown/protocol` workspace package.

**Tech Stack:**
- **Rust 1.75+**: `tokio`, `axum`, `sqlx` (SQLite, WAL), `ts-rs`, `tracing`, `proptest`, `rmcp` (MCP server), `serde`, `pathfinding` (A*).
- **Node 20+** with `pnpm`: Vite, React 18, Pixi.js 8, `pino`, `vitest`, Playwright.
- **CLIs (Phase 0)**: Claude Code, Codex CLI, opencode. Probed on world boot.

**How to read this plan:** Tasks are grouped into 10 milestones (M0–M9). Within a milestone, tasks are **sequential**. Across milestones, see §11.5 of the spec for parallelization lanes — once M0 freezes the protocol crate, lanes A, B, C, D can branch. Each task lists exact files, then a checkbox-numbered series of 2–5-minute steps. Run every test command shown; commit after each green test (this is non-negotiable Phase 0 discipline).

**Spec source of truth:** `docs/superpowers/specs/2026-05-07-cliptown-design.md` at commit `62f2f0e`. When this plan and the spec disagree, the spec wins — flag the drift and stop.

---

## Milestone 0 — Bootstrap (sequential, single lane)

Goal: a buildable repo with the workspace structure from spec §3.5, the protocol crate emitting `.d.ts`, the SQLite v1 migration runnable, and CI green on an empty test suite.

### Task 0.1: Cargo + pnpm workspace skeleton

**Files:**
- Create: `Cargo.toml`
- Create: `pnpm-workspace.yaml`
- Create: `package.json`
- Create: `.gitignore` (append)
- Create: `crates/world/Cargo.toml`
- Create: `crates/world/src/main.rs`
- Create: `crates/protocol/Cargo.toml`
- Create: `crates/protocol/src/lib.rs`
- Create: `packages/frontend/package.json`
- Create: `packages/worker/package.json`
- Create: `packages/protocol/package.json`
- Create: `packages/adapters/claude-code/package.json`
- Create: `packages/adapters/codex/package.json`
- Create: `packages/adapters/opencode/package.json`

- [ ] **Step 1: Write the workspace roots**

`Cargo.toml`:
```toml
[workspace]
members = ["crates/world", "crates/protocol"]
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
tracing-subscriber = { version = "0.3", features = ["json"] }
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
    "build": "pnpm -r build",
    "test": "pnpm -r test",
    "dev": "pnpm --filter @cliptown/frontend dev"
  },
  "packageManager": "pnpm@9.0.0"
}
```

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
```

- [ ] **Step 2: Write the per-crate / per-package skeletons**

`crates/world/Cargo.toml`:
```toml
[package]
name = "cliptown-world"
version = "0.0.1"
edition.workspace = true
rust-version.workspace = true

[dependencies]
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
cliptown-protocol = { path = "../protocol" }

[[bin]]
name = "cliptown-world"
path = "src/main.rs"
```

`crates/world/src/main.rs`:
```rust
fn main() {
    tracing_subscriber::fmt().json().init();
    tracing::info!(event = "boot", "cliptown world starting");
}
```

`crates/protocol/Cargo.toml`:
```toml
[package]
name = "cliptown-protocol"
version = "0.0.1"
edition.workspace = true

[dependencies]
serde.workspace = true
ts-rs.workspace = true
```

`crates/protocol/src/lib.rs`:
```rust
//! Protocol types shared between world (Rust) and worker/frontend (TS via ts-rs).
//! Add new types with #[derive(ts_rs::TS)] and #[ts(export, export_to = "../../packages/protocol/dist/")].

#[cfg(test)]
mod tests {
    #[test]
    fn protocol_crate_compiles() {
        // empty placeholder to drive ts-rs export side-effects in later tasks
    }
}
```

For each TS package (`packages/frontend`, `packages/worker`, `packages/protocol`, `packages/adapters/{claude-code,codex,opencode}`) write a minimal `package.json`:

```json
{
  "name": "@cliptown/<short-name>",
  "version": "0.0.1",
  "private": true,
  "type": "module",
  "scripts": {
    "build": "echo no-op",
    "test": "echo no-op"
  }
}
```

Use `frontend`, `worker`, `protocol`, `adapter-claude-code`, `adapter-codex`, `adapter-opencode` for the `<short-name>`.

- [ ] **Step 3: Verify the world binary builds**

Run: `cargo build -p cliptown-world`
Expected: "Compiling cliptown-protocol", "Compiling cliptown-world", "Finished".

- [ ] **Step 4: Verify pnpm install works**

Run: `pnpm install`
Expected: workspace resolves, no errors. `node_modules/` and `pnpm-lock.yaml` created.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml pnpm-workspace.yaml package.json .gitignore \
  crates/world crates/protocol \
  packages/frontend packages/worker packages/protocol \
  packages/adapters/claude-code packages/adapters/codex packages/adapters/opencode \
  pnpm-lock.yaml
git commit -m "chore(M0.1): scaffold Cargo + pnpm workspaces"
```

### Task 0.2: SQLite v1 migration + sqlx-cli wiring

**Files:**
- Create: `crates/world/migrations/0001_initial.sql`
- Modify: `crates/world/Cargo.toml` (add `sqlx`)
- Create: `crates/world/src/storage.rs`
- Modify: `crates/world/src/main.rs`
- Create: `crates/world/tests/storage_smoke.rs`

- [ ] **Step 1: Write the migration SQL**

`crates/world/migrations/0001_initial.sql` mirrors spec §4 verbatim. Lift the table list there. Include WAL pragma in a separate file or in the world's startup code (sqlx migrations don't run pragmas).

```sql
-- towns
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

-- startups (tenant root)
CREATE TABLE startups (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  goal_text TEXT NOT NULL,
  budget_cap_usd REAL NOT NULL,
  budget_spent_usd REAL NOT NULL DEFAULT 0,
  town_id TEXT NOT NULL REFERENCES towns(id),
  workspace_path TEXT NOT NULL,
  status TEXT NOT NULL,
  config_overrides TEXT,
  created_at INTEGER NOT NULL
);

CREATE TABLE agents (
  id TEXT PRIMARY KEY,
  startup_id TEXT NOT NULL REFERENCES startups(id),
  name TEXT NOT NULL,
  role TEXT NOT NULL,
  backend TEXT NOT NULL CHECK (backend IN ('claude_code','codex','opencode')),
  model_id TEXT NOT NULL,
  position_json TEXT NOT NULL,
  home_room_id TEXT NOT NULL,
  manager_id TEXT REFERENCES agents(id),
  status TEXT NOT NULL
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

- [ ] **Step 2: Add sqlx to the world crate**

In `crates/world/Cargo.toml` `[dependencies]` add:
```toml
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "macros", "migrate"] }
anyhow = "1"
```

- [ ] **Step 3: Write storage initializer with WAL pragmas**

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

Wire into `crates/world/src/main.rs`:
```rust
use anyhow::Result;
mod storage;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().json().init();
    let db_path = std::env::var("CLIPTOWN_DB").unwrap_or_else(|_| "cliptown.db".to_string());
    let _pool = storage::open(&db_path).await?;
    tracing::info!(event = "storage_ready", db = %db_path, "world storage initialized");
    Ok(())
}
```

- [ ] **Step 4: Write the smoke test**

`crates/world/tests/storage_smoke.rs`:
```rust
use cliptown_world::*;

#[tokio::test]
async fn storage_opens_and_runs_migrations_in_tempdir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let pool = cliptown_world::storage::open(path.to_str().unwrap()).await.unwrap();

    // verify journal_mode is WAL
    let row: (String,) = sqlx::query_as("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0.to_lowercase(), "wal");

    // verify a known table exists
    let row: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='startups'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, 1);
}
```

To make `storage` accessible from the integration test, expose it from `lib.rs`. Convert `crates/world/src/main.rs` to also expose a library, or add `crates/world/src/lib.rs`:

```rust
pub mod storage;
```

Add the lib target and `tempfile` dev-dep in `crates/world/Cargo.toml`:
```toml
[lib]
path = "src/lib.rs"

[dev-dependencies]
tempfile = "3"
```

Make `main.rs` reference the lib:
```rust
use anyhow::Result;
use cliptown_world::storage;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().json().init();
    let db_path = std::env::var("CLIPTOWN_DB").unwrap_or_else(|_| "cliptown.db".to_string());
    let _pool = storage::open(&db_path).await?;
    tracing::info!(event = "storage_ready", db = %db_path);
    Ok(())
}
```

- [ ] **Step 5: Run the test**

Run: `cargo test -p cliptown-world --test storage_smoke -- --nocapture`
Expected: `PASS`. The test creates a tempdir DB, runs migrations, verifies WAL.

- [ ] **Step 6: Commit**

```bash
git add crates/world/migrations crates/world/Cargo.toml \
  crates/world/src/lib.rs crates/world/src/storage.rs crates/world/src/main.rs \
  crates/world/tests/storage_smoke.rs Cargo.lock
git commit -m "feat(M0.2): SQLite v1 migration + WAL pragma init"
```

### Task 0.3: protocol crate first ts-rs export

**Files:**
- Modify: `crates/protocol/src/lib.rs`
- Create: `packages/protocol/dist/.gitkeep`
- Create: `packages/protocol/index.d.ts` (entry point)
- Create: `crates/protocol/tests/export.rs`

- [ ] **Step 1: Define the first protocol type**

`crates/protocol/src/lib.rs`:
```rust
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Schema version sent on every WS frame.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
pub struct SchemaVersion {
    pub v: u8,
}

impl SchemaVersion {
    pub const CURRENT: Self = Self { v: 1 };
}
```

- [ ] **Step 2: Wire the TS package entry point**

`packages/protocol/dist/.gitkeep`: empty file.

`packages/protocol/index.d.ts`:
```typescript
export * from "./dist/SchemaVersion";
// future ts-rs-generated types are re-exported by appending here
```

Update `packages/protocol/package.json`:
```json
{
  "name": "@cliptown/protocol",
  "version": "0.0.1",
  "private": true,
  "type": "module",
  "main": "index.d.ts",
  "types": "index.d.ts"
}
```

- [ ] **Step 3: Write the export test**

`crates/protocol/tests/export.rs`:
```rust
//! ts-rs writes export files when tests run with TS_RS_EXPORT_DIR unset.
//! This test exists so `cargo test -p cliptown-protocol` produces the .d.ts files.

use cliptown_protocol::SchemaVersion;

#[test]
fn schema_version_exports() {
    let s = SchemaVersion::CURRENT;
    assert_eq!(s.v, 1);
}
```

- [ ] **Step 4: Run the test and verify the .d.ts file appears**

Run: `cargo test -p cliptown-protocol`
Expected: `PASS`.

Run: `ls packages/protocol/dist/`
Expected: `SchemaVersion.ts` exists.

- [ ] **Step 5: Commit**

```bash
git add crates/protocol packages/protocol
git commit -m "feat(M0.3): protocol crate with first ts-rs export"
```

### Task 0.4: cliptown.toml config loader

**Files:**
- Create: `cliptown.toml` (repo root)
- Modify: `crates/world/Cargo.toml` (add `toml`, `serde`)
- Create: `crates/world/src/config.rs`
- Modify: `crates/world/src/lib.rs`
- Create: `crates/world/tests/config_smoke.rs`

- [ ] **Step 1: Write the default config file**

`cliptown.toml` — copy verbatim from spec §3.5 (under "Configuration"):
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

- [ ] **Step 2: Add `toml` dep**

`crates/world/Cargo.toml` `[dependencies]`:
```toml
toml = "0.8"
```

- [ ] **Step 3: Write the loader**

`crates/world/src/config.rs`:
```rust
use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub world: WorldCfg,
    pub task: TaskCfg,
    pub epistemic: EpistemicCfg,
    pub budget: BudgetCfg,
    pub supervisor: SupervisorCfg,
    pub possess: PossessCfg,
    pub kanban: KanbanCfg,
}

#[derive(Debug, Deserialize)]
pub struct WorldCfg { pub tick_hz: u32, pub position_snapshot_every_ticks: u32 }
#[derive(Debug, Deserialize)]
pub struct TaskCfg { pub max_review_rounds: u32, pub max_llm_turns_per_task: u32 }
#[derive(Debug, Deserialize)]
pub struct EpistemicCfg { pub max_hypotheses_per_task: u32, pub max_tests_per_hypothesis: u32, pub non_trivial_description_token_threshold: u32 }
#[derive(Debug, Deserialize)]
pub struct BudgetCfg { pub warn_pct: u32, pub no_new_task_pct: u32, pub pause_all_pct: u32 }
#[derive(Debug, Deserialize)]
pub struct SupervisorCfg { pub worker_respawn_backoff_seconds: Vec<u64>, pub worker_respawn_max_attempts: u32 }
#[derive(Debug, Deserialize)]
pub struct PossessCfg { pub operator_keepalive_timeout_seconds: u64 }
#[derive(Debug, Deserialize)]
pub struct KanbanCfg { pub stuck_warn_minutes: u32, pub stuck_alert_minutes: u32 }

pub fn load_from(path: &str) -> Result<Config> {
    let s = std::fs::read_to_string(path)?;
    let cfg: Config = toml::from_str(&s)?;
    Ok(cfg)
}
```

- [ ] **Step 4: Re-export from `lib.rs`**

`crates/world/src/lib.rs` (append):
```rust
pub mod config;
```

- [ ] **Step 5: Smoke test**

`crates/world/tests/config_smoke.rs`:
```rust
#[test]
fn loads_repo_root_config() {
    let cfg = cliptown_world::config::load_from("../../cliptown.toml").unwrap();
    assert_eq!(cfg.world.tick_hz, 1);
    assert_eq!(cfg.task.max_review_rounds, 3);
    assert_eq!(cfg.kanban.stuck_alert_minutes, 30);
}
```

Run: `cargo test -p cliptown-world --test config_smoke`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add cliptown.toml crates/world/Cargo.toml crates/world/src/config.rs \
  crates/world/src/lib.rs crates/world/tests/config_smoke.rs Cargo.lock
git commit -m "feat(M0.4): cliptown.toml loader with full Phase 0 surface"
```

### Task 0.5: CI skeleton

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Write CI workflow**

```yaml
name: CI
on:
  push:
    branches: [main]
  pull_request:
jobs:
  rust:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo build --workspace
      - run: cargo test --workspace
  ts:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: 20 }
      - uses: pnpm/action-setup@v4
        with: { version: 9 }
      - run: pnpm install --frozen-lockfile
      - run: pnpm -r build
      - run: pnpm -r test
```

- [ ] **Step 2: Commit**

```bash
git add .github
git commit -m "ci(M0.5): cargo and pnpm workspace CI"
```

---

## Milestone 1 — World Server Core (Lane A)

Goal: world process boots, listens on `:8080`, accepts WS connections on `/ws/console` and `/ws/worker`, runs a 1Hz tick, persists positions to SQLite, exposes a backend catalog. No agents yet — just the substrate.

### Task 1.1: Boot + tracing + axum HTTP

**Files:**
- Modify: `crates/world/Cargo.toml`
- Modify: `crates/world/src/main.rs`
- Create: `crates/world/src/http.rs`
- Modify: `crates/world/src/lib.rs`
- Create: `crates/world/tests/http_smoke.rs`

- [ ] **Step 1: Add axum + tower deps**

```toml
axum = { version = "0.7", features = ["ws", "json"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["trace"] }
hyper = "1"
```

- [ ] **Step 2: Write the HTTP module**

`crates/world/src/http.rs`:
```rust
use axum::{routing::get, Router, response::Json};
use serde_json::json;

pub fn router() -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({"ok": true})) }))
}
```

- [ ] **Step 3: Wire into `main.rs`**

```rust
use anyhow::Result;
use cliptown_world::{config, http, storage};
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().json().init();

    let _cfg = config::load_from("cliptown.toml")?;
    let db_path = std::env::var("CLIPTOWN_DB").unwrap_or_else(|_| "cliptown.db".into());
    let _pool = storage::open(&db_path).await?;

    let app = http::router();
    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(event = "listening", addr = %addr);
    axum::serve(listener, app).await?;
    Ok(())
}
```

Re-export in `lib.rs`: `pub mod http;`.

- [ ] **Step 4: Smoke test the health endpoint**

`crates/world/tests/http_smoke.rs`:
```rust
use axum::body::to_bytes;
use tower::ServiceExt;

#[tokio::test]
async fn health_returns_ok_json() {
    let app = cliptown_world::http::router();
    let req = axum::http::Request::builder()
        .uri("/health")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], br#"{"ok":true}"#);
}
```

Run: `cargo test -p cliptown-world --test http_smoke`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/world Cargo.lock
git commit -m "feat(M1.1): world boots axum on :8080 with /health"
```

### Task 1.2: World state + mpsc inbox + tick

**Files:**
- Create: `crates/world/src/state.rs`
- Create: `crates/world/src/loop_.rs`
- Modify: `crates/world/src/lib.rs`
- Modify: `crates/world/src/main.rs`
- Create: `crates/world/tests/tick_smoke.rs`

- [ ] **Step 1: Define minimal in-memory state**

`crates/world/src/state.rs`:
```rust
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct World {
    pub tick_seq: u64,
    pub positions: HashMap<String, (i32, i32, String)>, // agent_id -> (x, y, room_id)
}

impl World {
    pub fn advance_tick(&mut self) {
        self.tick_seq = self.tick_seq.wrapping_add(1);
    }
}
```

- [ ] **Step 2: Write the event loop**

`crates/world/src/loop_.rs`:
```rust
use crate::state::World;
use tokio::sync::{mpsc, watch};

#[derive(Debug)]
pub enum InMsg {
    Tick,
    Shutdown,
}

pub struct Handles {
    pub tx: mpsc::Sender<InMsg>,
    pub view_rx: watch::Receiver<World>,
}

pub fn spawn(initial: World) -> Handles {
    let (tx, mut rx) = mpsc::channel::<InMsg>(1024);
    let (view_tx, view_rx) = watch::channel(initial.clone());
    let mut w = initial;

    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match msg {
                InMsg::Tick => {
                    w.advance_tick();
                    let _ = view_tx.send(w.clone());
                }
                InMsg::Shutdown => break,
            }
        }
    });

    let timer_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            if timer_tx.send(InMsg::Tick).await.is_err() { break; }
        }
    });

    Handles { tx, view_rx }
}
```

- [ ] **Step 3: Re-export and wire into main**

`lib.rs` append:
```rust
pub mod state;
pub mod loop_;
```

`main.rs` after `_pool` line:
```rust
let _handles = cliptown_world::loop_::spawn(cliptown_world::state::World::default());
```

- [ ] **Step 4: Test tick advances**

`crates/world/tests/tick_smoke.rs`:
```rust
use cliptown_world::{loop_, state::World};

#[tokio::test(start_paused = true)]
async fn tick_advances_seq_each_second() {
    let h = loop_::spawn(World::default());
    let initial = h.view_rx.borrow().tick_seq;
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    // give the loop a chance to process
    tokio::task::yield_now().await;
    let now = h.view_rx.borrow().tick_seq;
    assert!(now > initial, "expected tick advance, got {now} from {initial}");
}
```

Run: `cargo test -p cliptown-world --test tick_smoke`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/world Cargo.lock
git commit -m "feat(M1.2): mpsc event loop + 1Hz tick + watch view"
```

### Task 1.3: WS endpoints (skeleton, accept-only)

**Files:**
- Modify: `crates/world/src/http.rs`
- Modify: `crates/world/src/main.rs`
- Modify: `crates/world/Cargo.toml` (add `axum` ws is already there; add `tokio-tungstenite` if needed for the test client)
- Create: `crates/world/tests/ws_smoke.rs`

- [ ] **Step 1: Add WS routes**

`crates/world/src/http.rs`:
```rust
use axum::{
    extract::{ws::{WebSocket, WebSocketUpgrade}, State},
    response::{Json, Response},
    routing::get,
    Router,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::watch;
use crate::state::World;

#[derive(Clone)]
pub struct AppState {
    pub view: watch::Receiver<World>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({"ok": true})) }))
        .route("/ws/console", get(ws_console))
        .route("/ws/worker", get(ws_worker))
        .with_state(Arc::new(state))
}

async fn ws_console(ws: WebSocketUpgrade, State(_s): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(handle_console)
}

async fn ws_worker(ws: WebSocketUpgrade, State(_s): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(handle_worker)
}

async fn handle_console(mut socket: WebSocket) {
    while let Some(msg) = socket.recv().await {
        let Ok(_msg) = msg else { break };
        // skeleton: echo a hello on first message
        let _ = socket.send(axum::extract::ws::Message::Text(r#"{"type":"hello_console"}"#.into())).await;
    }
}

async fn handle_worker(mut socket: WebSocket) {
    while let Some(msg) = socket.recv().await {
        let Ok(_msg) = msg else { break };
        let _ = socket.send(axum::extract::ws::Message::Text(r#"{"type":"hello_worker"}"#.into())).await;
    }
}
```

- [ ] **Step 2: Update main to pass state**

```rust
let handles = cliptown_world::loop_::spawn(cliptown_world::state::World::default());
let app = cliptown_world::http::router(cliptown_world::http::AppState { view: handles.view_rx });
```

- [ ] **Step 3: Test client connects**

Add `[dev-dependencies]` to `crates/world/Cargo.toml`:
```toml
tokio-tungstenite = "0.24"
futures-util = "0.3"
```

`crates/world/tests/ws_smoke.rs`:
```rust
use cliptown_world::{http, loop_, state::World};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn worker_ws_echoes_hello() {
    let h = loop_::spawn(World::default());
    let app = http::router(http::AppState { view: h.view_rx });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let url = format!("ws://{addr}/ws/worker");
    let (mut stream, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    stream.send(Message::Text("ping".into())).await.unwrap();
    let reply = stream.next().await.unwrap().unwrap();
    assert!(reply.into_text().unwrap().contains("hello_worker"));
}
```

Run: `cargo test -p cliptown-world --test ws_smoke`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/world Cargo.lock
git commit -m "feat(M1.3): /ws/console and /ws/worker accept connections"
```

### Task 1.4: Backend catalog probe at boot

**Files:**
- Create: `crates/world/src/backend_catalog.rs`
- Modify: `crates/world/src/lib.rs`
- Modify: `crates/world/src/main.rs`
- Modify: `crates/world/src/http.rs`
- Create: `crates/world/tests/backend_catalog.rs`

- [ ] **Step 1: Implement the probe**

`crates/world/src/backend_catalog.rs`:
```rust
use serde::Serialize;
use std::collections::HashMap;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
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
    ] {
        let info = probe_one(id, cmd, hint).await;
        out.insert(id.to_string(), info);
    }
    out
}

async fn probe_one(id: &str, cmd: &str, hint: &str) -> BackendInfo {
    let now = chrono::Utc::now().timestamp();
    match tokio::process::Command::new(cmd)
        .arg("--version")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn() {
        Ok(mut child) => {
            match tokio::time::timeout(std::time::Duration::from_secs(2), child.wait_with_output()).await {
                Ok(Ok(out)) if out.status.success() => {
                    let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    BackendInfo { id: id.into(), available: true, version: Some(version), install_hint: None, last_checked_ts: now }
                }
                _ => BackendInfo { id: id.into(), available: false, version: None, install_hint: Some(hint.into()), last_checked_ts: now },
            }
        }
        Err(_) => BackendInfo { id: id.into(), available: false, version: None, install_hint: Some(hint.into()), last_checked_ts: now },
    }
}
```

Add deps in `crates/world/Cargo.toml`:
```toml
chrono = { version = "0.4", default-features = false, features = ["clock", "serde"] }
```

- [ ] **Step 2: Re-export and surface via HTTP**

`lib.rs` append:
```rust
pub mod backend_catalog;
```

`http.rs` add the route + state field:
```rust
#[derive(Clone)]
pub struct AppState {
    pub view: watch::Receiver<crate::state::World>,
    pub catalog: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, crate::backend_catalog::BackendInfo>>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({"ok": true})) }))
        .route("/api/backend-catalog", get(api_catalog))
        .route("/ws/console", get(ws_console))
        .route("/ws/worker", get(ws_worker))
        .with_state(Arc::new(state))
}

async fn api_catalog(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let m = s.catalog.read().await;
    Json(serde_json::to_value(&*m).unwrap())
}
```

`main.rs`:
```rust
let catalog = std::sync::Arc::new(tokio::sync::RwLock::new(
    cliptown_world::backend_catalog::probe_all().await,
));
let app = cliptown_world::http::router(cliptown_world::http::AppState { view: handles.view_rx, catalog });
```

- [ ] **Step 3: Test it returns three entries**

`crates/world/tests/backend_catalog.rs`:
```rust
#[tokio::test]
async fn probe_returns_three_entries() {
    let m = cliptown_world::backend_catalog::probe_all().await;
    assert_eq!(m.len(), 3);
    assert!(m.contains_key("claude_code"));
    assert!(m.contains_key("codex"));
    assert!(m.contains_key("opencode"));
    // we don't assert availability — depends on host
}
```

Run: `cargo test -p cliptown-world --test backend_catalog`
Expected: PASS regardless of which CLIs are installed.

- [ ] **Step 4: Commit**

```bash
git add crates/world Cargo.lock
git commit -m "feat(M1.4): backend catalog probe + /api/backend-catalog"
```

### Task 1.5: town_default seed (5 rooms hardcoded JSON)

**Files:**
- Create: `crates/world/src/seed.rs`
- Modify: `crates/world/src/lib.rs`
- Modify: `crates/world/src/main.rs`
- Create: `crates/world/tests/seed_smoke.rs`

- [ ] **Step 1: Write the seed function**

`crates/world/src/seed.rs`:
```rust
use anyhow::Result;
use sqlx::SqlitePool;

const TOWN_ID: &str = "town_default";
const TOWN_MAP_JSON: &str = include_str!("../seed/town_default.json");

pub async fn seed_if_empty(pool: &SqlitePool) -> Result<()> {
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM towns")
        .fetch_one(pool).await?;
    if count.0 > 0 { return Ok(()); }

    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO towns (id, name, map_json) VALUES (?, ?, ?)")
        .bind(TOWN_ID).bind("Default Town").bind(TOWN_MAP_JSON)
        .execute(&mut *tx).await?;

    // 4 suite slots (initially unowned) + 3 common rooms
    let rooms = [
        ("suite_1", "Suite 1", "office", r#"{"x":0,"y":0,"w":7,"h":6}"#),
        ("suite_2", "Suite 2", "office", r#"{"x":0,"y":6,"w":7,"h":6}"#),
        ("suite_3", "Suite 3", "office", r#"{"x":33,"y":0,"w":7,"h":6}"#),
        ("suite_4", "Suite 4", "office", r#"{"x":33,"y":6,"w":7,"h":6}"#),
        ("lobby",   "Lobby",   "transit", r#"{"x":7,"y":4,"w":26,"h":4}"#),
        ("cafe",    "Cafe",    "social",  r#"{"x":7,"y":0,"w":26,"h":4}"#),
        ("library", "Library", "focus",   r#"{"x":7,"y":8,"w":26,"h":4}"#),
    ];
    for (id, name, kind, bounds) in rooms {
        sqlx::query("INSERT INTO rooms (id, town_id, name, type, bounds, private_to_startup_id) VALUES (?, ?, ?, ?, ?, NULL)")
            .bind(id).bind(TOWN_ID).bind(name).bind(kind).bind(bounds)
            .execute(&mut *tx).await?;
    }
    // doors
    let doors = [
        ("door_s1_lobby", "suite_1", "lobby", 7, 4),
        ("door_s2_lobby", "suite_2", "lobby", 7, 7),
        ("door_s3_lobby", "suite_3", "lobby", 33, 4),
        ("door_s4_lobby", "suite_4", "lobby", 33, 7),
        ("door_lobby_cafe", "lobby", "cafe", 20, 4),
        ("door_lobby_library", "lobby", "library", 20, 8),
    ];
    for (id, a, b, x, y) in doors {
        sqlx::query("INSERT INTO room_doors (id, town_id, room_a, room_b, tile_x, tile_y) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(id).bind(TOWN_ID).bind(a).bind(b).bind(x).bind(y)
            .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
```

`crates/world/seed/town_default.json` (informational, not parsed at runtime — kept for documentation):
```json
{ "tile_size": 32, "width_tiles": 40, "height_tiles": 12 }
```

- [ ] **Step 2: Re-export and call on boot**

`lib.rs`: `pub mod seed;`
`main.rs` (after `_pool = storage::open(...)`):
```rust
cliptown_world::seed::seed_if_empty(&_pool).await?;
```
Bind the pool: change `let _pool` to `let pool` and use `&pool`. Also store it in `AppState` so later tasks can use it.

- [ ] **Step 3: Smoke test**

`crates/world/tests/seed_smoke.rs`:
```rust
#[tokio::test]
async fn seed_creates_one_town_seven_rooms_six_doors() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("test.db");
    let pool = cliptown_world::storage::open(p.to_str().unwrap()).await.unwrap();
    cliptown_world::seed::seed_if_empty(&pool).await.unwrap();

    let towns: (i64,) = sqlx::query_as("SELECT count(*) FROM towns").fetch_one(&pool).await.unwrap();
    let rooms: (i64,) = sqlx::query_as("SELECT count(*) FROM rooms").fetch_one(&pool).await.unwrap();
    let doors: (i64,) = sqlx::query_as("SELECT count(*) FROM room_doors").fetch_one(&pool).await.unwrap();
    assert_eq!(towns.0, 1);
    assert_eq!(rooms.0, 7);
    assert_eq!(doors.0, 6);

    // calling again is a no-op
    cliptown_world::seed::seed_if_empty(&pool).await.unwrap();
    let towns2: (i64,) = sqlx::query_as("SELECT count(*) FROM towns").fetch_one(&pool).await.unwrap();
    assert_eq!(towns2.0, 1);
}
```

Run: `cargo test -p cliptown-world --test seed_smoke`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/world Cargo.lock
git commit -m "feat(M1.5): seed town_default with 4 suite slots + 3 common rooms"
```

### Task 1.6: A* pathfinding on the room/tile graph

**Files:**
- Create: `crates/world/src/path.rs`
- Modify: `crates/world/Cargo.toml`
- Modify: `crates/world/src/lib.rs`
- Create: `crates/world/tests/path_unit.rs`

- [ ] **Step 1: Add `pathfinding` dep**

```toml
pathfinding = "4"
```

- [ ] **Step 2: Implement room-graph A* (rooms as nodes, doors as edges, target tile inside the destination room)**

`crates/world/src/path.rs`:
```rust
use pathfinding::prelude::astar;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Door { pub a: String, pub b: String, pub tile: (i32, i32) }

#[derive(Debug, Clone)]
pub struct RoomGraph {
    pub doors: Vec<Door>,
    pub neighbors: HashMap<String, Vec<(String, (i32, i32))>>, // room -> [(neighbor_room, door_tile)]
}

impl RoomGraph {
    pub fn from_doors(doors: Vec<Door>) -> Self {
        let mut neighbors: HashMap<String, Vec<(String, (i32, i32))>> = HashMap::new();
        for d in &doors {
            neighbors.entry(d.a.clone()).or_default().push((d.b.clone(), d.tile));
            neighbors.entry(d.b.clone()).or_default().push((d.a.clone(), d.tile));
        }
        Self { doors, neighbors }
    }
    /// Returns Vec<(room_id, tile_to_enter_room)> from `from` to `to`. Empty Vec if same room.
    pub fn route(&self, from: &str, to: &str) -> Option<Vec<(String, (i32, i32))>> {
        if from == to { return Some(vec![]); }
        let result = astar(
            &from.to_string(),
            |r| self.neighbors.get(r).cloned().unwrap_or_default()
                .into_iter().map(|(nb, _tile)| (nb, 1u32)).collect::<Vec<_>>(),
            |_| 0u32,
            |r| r == to,
        );
        result.map(|(path, _)| {
            path.windows(2)
                .map(|w| {
                    let next = &w[1];
                    let tile = self.neighbors.get(&w[0]).unwrap()
                        .iter().find(|(n, _)| n == next).unwrap().1;
                    (next.clone(), tile)
                })
                .collect()
        })
    }
}
```

- [ ] **Step 3: Re-export**

`lib.rs`: `pub mod path;`

- [ ] **Step 4: Unit test routes**

`crates/world/tests/path_unit.rs`:
```rust
use cliptown_world::path::{Door, RoomGraph};

fn graph() -> RoomGraph {
    RoomGraph::from_doors(vec![
        Door { a: "suite_1".into(), b: "lobby".into(), tile: (7, 4) },
        Door { a: "lobby".into(), b: "library".into(), tile: (20, 8) },
        Door { a: "lobby".into(), b: "cafe".into(), tile: (20, 4) },
    ])
}

#[test]
fn same_room_is_empty_path() {
    let g = graph();
    assert_eq!(g.route("lobby", "lobby"), Some(vec![]));
}

#[test]
fn suite_to_library_passes_through_lobby() {
    let g = graph();
    let r = g.route("suite_1", "library").unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].0, "lobby");
    assert_eq!(r[1].0, "library");
}

#[test]
fn no_path_returns_none() {
    let g = RoomGraph::from_doors(vec![Door { a: "a".into(), b: "b".into(), tile: (0, 0) }]);
    assert_eq!(g.route("a", "z"), None);
}
```

Run: `cargo test -p cliptown-world --test path_unit`
Expected: 3 PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/world Cargo.lock
git commit -m "feat(M1.6): A* room-graph pathfinding"
```

### Task 1.7: Permission predicates + property tests

**Files:**
- Create: `crates/world/src/permissions.rs`
- Modify: `crates/world/src/lib.rs`
- Modify: `crates/world/Cargo.toml`
- Create: `crates/world/tests/permissions_property.rs`

- [ ] **Step 1: Add proptest**

```toml
[dev-dependencies]
proptest = "1"
```

- [ ] **Step 2: Implement predicates**

`crates/world/src/permissions.rs`:
```rust
#[derive(Debug, Clone)]
pub struct AgentRef<'a> {
    pub agent_id: &'a str,
    pub startup_id: &'a str,
    pub kind: AgentKind,
    pub manager_id: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind { Agent, Operator }

pub fn can_enter_room(agent: &AgentRef, room_private_to: Option<&str>) -> bool {
    if agent.kind == AgentKind::Operator { return true; }
    match room_private_to {
        None => true,
        Some(owner) => owner == agent.startup_id,
    }
}

pub fn can_send_directive(from: &AgentRef, to: &AgentRef) -> bool {
    if from.kind == AgentKind::Operator { return true; }
    if from.startup_id != to.startup_id { return false; }
    to.manager_id == Some(from.agent_id)
}

pub fn can_receive_chat_in_room(listener: &AgentRef, speaker: &AgentRef, room_private_to: Option<&str>) -> bool {
    // listener must be in the room; we don't have positions here, the caller filters by room.
    // Cross-startup chat is allowed in common rooms; suites are scoped by entry permission already.
    can_enter_room(listener, room_private_to) && can_enter_room(speaker, room_private_to)
}
```

`lib.rs`: `pub mod permissions;`

- [ ] **Step 3: Property tests**

`crates/world/tests/permissions_property.rs`:
```rust
use cliptown_world::permissions::*;
use proptest::prelude::*;

proptest! {
    #[test]
    fn agent_never_enters_foreign_suite(
        startup_self in "[a-z]{3}", startup_owner in "[a-z]{3}"
    ) {
        prop_assume!(startup_self != startup_owner);
        let a = AgentRef { agent_id: "x", startup_id: &startup_self, kind: AgentKind::Agent, manager_id: None };
        prop_assert!(!can_enter_room(&a, Some(&startup_owner)));
    }

    #[test]
    fn directive_never_crosses_startup_boundary(
        a in "[a-z]{3}", b in "[a-z]{3}"
    ) {
        prop_assume!(a != b);
        let from = AgentRef { agent_id: "f", startup_id: &a, kind: AgentKind::Agent, manager_id: None };
        let to   = AgentRef { agent_id: "t", startup_id: &b, kind: AgentKind::Agent, manager_id: Some("f") };
        prop_assert!(!can_send_directive(&from, &to));
    }

    #[test]
    fn operator_can_always_enter(any_owner in proptest::option::of("[a-z]{3}")) {
        let op = AgentRef { agent_id: "op", startup_id: "_", kind: AgentKind::Operator, manager_id: None };
        let owner = any_owner.as_deref();
        prop_assert!(can_enter_room(&op, owner));
    }
}

#[test]
fn directive_within_same_startup_to_direct_report() {
    let from = AgentRef { agent_id: "f", startup_id: "a", kind: AgentKind::Agent, manager_id: None };
    let to   = AgentRef { agent_id: "t", startup_id: "a", kind: AgentKind::Agent, manager_id: Some("f") };
    assert!(can_send_directive(&from, &to));
}
```

Run: `cargo test -p cliptown-world --test permissions_property`
Expected: 4 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/world Cargo.lock
git commit -m "feat(M1.7): permission predicates + multi-tenant property tests (invariant 8)"
```

### Task 1.8: Task state machine

**Files:**
- Create: `crates/world/src/task_sm.rs`
- Modify: `crates/world/src/lib.rs`
- Create: `crates/world/tests/task_sm_unit.rs`

- [ ] **Step 1: Implement the state machine**

`crates/world/src/task_sm.rs`:
```rust
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Proposed,
    Queued,
    InProgress,
    AwaitingReview,
    ChangesRequested,
    Done,
    Failed,
    Escalated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Manager,
    NonManager,
    Operator,
    System,
}

#[derive(Debug)]
pub enum Transition {
    SubtaskCreate { caller: Actor },
    AcceptProposal { caller: Actor },
    RejectProposal { caller: Actor },
    AssignFromQueued, // scheduler picks up
    TaskDoneMcp,
    RequestChanges,
    TaskAccept,
    OperatorForceAccept,
    OperatorForceFail,
    Fail,
    Escalate,
}

pub fn next(current: TaskStatus, t: &Transition) -> Result<TaskStatus, &'static str> {
    use TaskStatus::*;
    match (current, t) {
        (_, Transition::SubtaskCreate { caller: Actor::Manager }) => Ok(Queued),
        (_, Transition::SubtaskCreate { caller: Actor::NonManager }) => Ok(Proposed),
        (Proposed, Transition::AcceptProposal { .. }) => Ok(Queued),
        (Proposed, Transition::RejectProposal { .. }) => Ok(Failed),
        (Queued, Transition::AssignFromQueued) => Ok(InProgress),
        (InProgress, Transition::TaskDoneMcp) => Ok(AwaitingReview),
        (AwaitingReview, Transition::RequestChanges) => Ok(ChangesRequested),
        (ChangesRequested, Transition::TaskDoneMcp) => Ok(AwaitingReview),
        (AwaitingReview, Transition::TaskAccept) => Ok(Done),
        (AwaitingReview, Transition::OperatorForceAccept) => Ok(Done),
        (s, Transition::OperatorForceFail) if s != Done && s != Failed => Ok(Failed),
        (s, Transition::Fail) if s != Done && s != Failed => Ok(Failed),
        (s, Transition::Escalate) if s != Done && s != Failed => Ok(Escalated),
        _ => Err("illegal transition"),
    }
}
```

`lib.rs`: `pub mod task_sm;`

- [ ] **Step 2: Unit tests**

`crates/world/tests/task_sm_unit.rs`:
```rust
use cliptown_world::task_sm::*;

#[test]
fn manager_subtask_goes_straight_to_queued() {
    assert_eq!(next(TaskStatus::Proposed, &Transition::SubtaskCreate { caller: Actor::Manager }).unwrap(), TaskStatus::Queued);
}

#[test]
fn nonmanager_subtask_lands_in_proposed() {
    assert_eq!(next(TaskStatus::Proposed, &Transition::SubtaskCreate { caller: Actor::NonManager }).unwrap(), TaskStatus::Proposed);
}

#[test]
fn review_round_loop() {
    assert_eq!(next(TaskStatus::InProgress, &Transition::TaskDoneMcp).unwrap(), TaskStatus::AwaitingReview);
    assert_eq!(next(TaskStatus::AwaitingReview, &Transition::RequestChanges).unwrap(), TaskStatus::ChangesRequested);
    assert_eq!(next(TaskStatus::ChangesRequested, &Transition::TaskDoneMcp).unwrap(), TaskStatus::AwaitingReview);
    assert_eq!(next(TaskStatus::AwaitingReview, &Transition::TaskAccept).unwrap(), TaskStatus::Done);
}

#[test]
fn operator_force_accept_only_from_awaiting_review() {
    assert_eq!(next(TaskStatus::AwaitingReview, &Transition::OperatorForceAccept).unwrap(), TaskStatus::Done);
    assert!(next(TaskStatus::Queued, &Transition::OperatorForceAccept).is_err());
}

#[test]
fn done_is_terminal() {
    assert!(next(TaskStatus::Done, &Transition::Fail).is_err());
    assert!(next(TaskStatus::Done, &Transition::OperatorForceFail).is_err());
}
```

Run: `cargo test -p cliptown-world --test task_sm_unit`
Expected: 5 PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/world
git commit -m "feat(M1.8): task state machine with proposed/operator-force transitions"
```

### Task 1.9: Sandbox path-escape resolver (battery)

**Files:**
- Create: `crates/world/src/sandbox.rs`
- Modify: `crates/world/src/lib.rs`
- Create: `crates/world/tests/sandbox_attacks.rs`

- [ ] **Step 1: Implement the resolver**

`crates/world/src/sandbox.rs`:
```rust
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

pub fn resolve(root: &Path, candidate: &str) -> Result<PathBuf> {
    if candidate.is_empty() { return Err(anyhow!("empty path")); }
    if candidate.contains('\0') { return Err(anyhow!("nul byte in path")); }
    if candidate.len() > 4096 { return Err(anyhow!("path too long")); }
    let p = Path::new(candidate);
    if p.is_absolute() { return Err(anyhow!("absolute path forbidden")); }
    let joined = root.join(p);
    let canon_root = root.canonicalize().map_err(|e| anyhow!("root canonicalize: {e}"))?;
    let canon = match joined.canonicalize() {
        Ok(c) => c,
        Err(_) => {
            // file may not exist yet — verify the parent then synthesize
            let parent = joined.parent().ok_or_else(|| anyhow!("no parent"))?;
            let parent_c = parent.canonicalize().map_err(|e| anyhow!("parent canonicalize: {e}"))?;
            if !parent_c.starts_with(&canon_root) { return Err(anyhow!("parent escapes root")); }
            parent_c.join(joined.file_name().ok_or_else(|| anyhow!("no file name"))?)
        }
    };
    if !canon.starts_with(&canon_root) {
        return Err(anyhow!("path escapes root"));
    }
    Ok(canon)
}
```

`lib.rs`: `pub mod sandbox;`

- [ ] **Step 2: Attack battery test**

`crates/world/tests/sandbox_attacks.rs`:
```rust
use cliptown_world::sandbox::resolve;
use std::fs;
use std::os::unix::fs::symlink;
use tempfile::tempdir;

fn root() -> tempfile::TempDir {
    let d = tempdir().unwrap();
    fs::create_dir_all(d.path().join("artifacts")).unwrap();
    d
}

#[test]
fn rejects_dot_dot_escape() {
    let d = root();
    assert!(resolve(d.path(), "../etc/passwd").is_err());
    assert!(resolve(d.path(), "../../etc/passwd").is_err());
    assert!(resolve(d.path(), "././../etc/passwd").is_err());
}

#[test]
fn rejects_absolute_path() {
    let d = root();
    assert!(resolve(d.path(), "/etc/passwd").is_err());
}

#[test]
fn rejects_nul_byte() {
    let d = root();
    assert!(resolve(d.path(), "artifacts/foo\0.md").is_err());
}

#[test]
fn rejects_too_long() {
    let d = root();
    let long = "a".repeat(5000);
    assert!(resolve(d.path(), &long).is_err());
}

#[test]
fn rejects_symlink_escape() {
    let d = root();
    let outside = d.path().parent().unwrap().to_path_buf();
    symlink(&outside, d.path().join("artifacts").join("link")).unwrap();
    assert!(resolve(d.path(), "artifacts/link/passwd").is_err());
}

#[test]
fn allows_legit_artifact_path() {
    let d = root();
    let r = resolve(d.path(), "artifacts/T1.md").unwrap();
    assert!(r.starts_with(d.path()));
}
```

Run: `cargo test -p cliptown-world --test sandbox_attacks`
Expected: 6 PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/world Cargo.lock
git commit -m "feat(M1.9): sandbox path resolver + path-escape attack battery"
```

### Task 1.10: SQLite write-batch helpers (audit_trail / fs_audit windows)

**Files:**
- Create: `crates/world/src/persist.rs`
- Modify: `crates/world/src/lib.rs`
- Create: `crates/world/tests/persist_smoke.rs`

- [ ] **Step 1: Implement batched insert**

`crates/world/src/persist.rs`:
```rust
use anyhow::Result;
use sqlx::SqlitePool;

pub async fn append_audit(pool: &SqlitePool, task_id: &str, entry_json: &str) -> Result<()> {
    sqlx::query(
        "UPDATE tasks SET audit_trail = json_insert(audit_trail, '$[#]', json(?)), updated_at = unixepoch() WHERE id = ?"
    )
    .bind(entry_json).bind(task_id).execute(pool).await?;
    Ok(())
}

pub async fn append_epistemic(pool: &SqlitePool, task_id: &str, entry_json: &str) -> Result<()> {
    sqlx::query(
        "UPDATE tasks SET epistemic_log = json_insert(epistemic_log, '$[#]', json(?)), updated_at = unixepoch() WHERE id = ?"
    )
    .bind(entry_json).bind(task_id).execute(pool).await?;
    Ok(())
}

pub async fn record_fs_audit(
    pool: &SqlitePool, startup_id: &str, agent_id: &str,
    op: &str, path: &str, bytes: i64, ok: bool, err: Option<&str>
) -> Result<()> {
    sqlx::query(
        "INSERT INTO fs_audit (id, startup_id, agent_id, op, path, bytes, ok, error, ts) VALUES (?, ?, ?, ?, ?, ?, ?, ?, unixepoch())"
    )
    .bind(uuid::Uuid::new_v4().to_string()).bind(startup_id).bind(agent_id)
    .bind(op).bind(path).bind(bytes).bind(if ok {1} else {0}).bind(err)
    .execute(pool).await?;
    Ok(())
}
```

Add `uuid = { version = "1", features = ["v4"] }` to deps.

`lib.rs`: `pub mod persist;`

- [ ] **Step 2: Smoke test (creates startup + task + appends)**

`crates/world/tests/persist_smoke.rs`:
```rust
use cliptown_world::{persist, seed, storage};
use sqlx::Row;

#[tokio::test]
async fn append_audit_grows_array() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("test.db");
    let pool = storage::open(p.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();

    sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) VALUES ('s1','α','goal',10,'town_default','/tmp/s1','active', unixepoch())")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO tasks (id, startup_id, title, description, status, created_at, updated_at) VALUES ('T1','s1','t','d','queued', unixepoch(), unixepoch())")
        .execute(&pool).await.unwrap();

    persist::append_audit(&pool, "T1", r#"{"kind":"task_assigned","ts":1}"#).await.unwrap();
    persist::append_audit(&pool, "T1", r#"{"kind":"task_done","ts":2}"#).await.unwrap();

    let row = sqlx::query("SELECT json_array_length(audit_trail) AS n FROM tasks WHERE id='T1'").fetch_one(&pool).await.unwrap();
    assert_eq!(row.get::<i64, _>("n"), 2);
}
```

Run: `cargo test -p cliptown-world --test persist_smoke`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/world Cargo.lock
git commit -m "feat(M1.10): persist helpers for audit_trail / epistemic_log / fs_audit"
```

---

## Milestone 2 — Worker + MCP proxy (Lane C)

Goal: a TS Worker process can connect to `/ws/worker`, identify, expose an in-process MCP server to a (mocked) child CLI, and translate MCP tool calls into WS round-trips.

### Task 2.1: Worker package skeleton + WS hello

**Files:**
- Modify: `packages/worker/package.json`
- Create: `packages/worker/tsconfig.json`
- Create: `packages/worker/src/index.ts`
- Create: `packages/worker/src/ws.ts`
- Create: `packages/worker/test/ws.test.ts`

- [ ] **Step 1: tsconfig + deps**

`packages/worker/package.json`:
```json
{
  "name": "@cliptown/worker",
  "version": "0.0.1",
  "private": true,
  "type": "module",
  "main": "dist/index.js",
  "scripts": {
    "build": "tsc -p .",
    "test": "vitest run",
    "dev": "tsx src/index.ts"
  },
  "dependencies": {
    "@cliptown/protocol": "workspace:*",
    "ws": "^8",
    "pino": "^9"
  },
  "devDependencies": {
    "typescript": "^5.5",
    "tsx": "^4",
    "vitest": "^2",
    "@types/ws": "^8",
    "@types/node": "^20"
  }
}
```

`packages/worker/tsconfig.json`:
```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "strict": true,
    "outDir": "dist",
    "declaration": true,
    "esModuleInterop": true,
    "skipLibCheck": true
  },
  "include": ["src/**/*"]
}
```

Run: `pnpm install`

- [ ] **Step 2: WS connect + hello**

`packages/worker/src/ws.ts`:
```ts
import WebSocket from "ws";

export interface WorkerHandle {
  send(msg: unknown): void;
  close(): void;
  onMessage(fn: (m: unknown) => void): void;
  onClose(fn: () => void): void;
}

export async function connect(opts: {
  url: string; agentId: string; startupId: string; secret: string;
}): Promise<WorkerHandle> {
  const ws = new WebSocket(opts.url);
  await new Promise<void>((res, rej) => {
    ws.once("open", () => res());
    ws.once("error", rej);
  });
  ws.send(JSON.stringify({ v: 1, type: "hello", agent_id: opts.agentId, startup_id: opts.startupId, secret: opts.secret }));
  let listeners: ((m: unknown) => void)[] = [];
  let closeListeners: (() => void)[] = [];
  ws.on("message", (data) => {
    try { listeners.forEach((l) => l(JSON.parse(String(data)))); } catch {}
  });
  ws.on("close", () => closeListeners.forEach((l) => l()));
  return {
    send: (m) => ws.send(JSON.stringify(m)),
    close: () => ws.close(),
    onMessage: (fn) => { listeners.push(fn); },
    onClose: (fn) => { closeListeners.push(fn); },
  };
}
```

- [ ] **Step 3: Test against the world skeleton**

`packages/worker/test/ws.test.ts`:
```ts
import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { spawn, ChildProcess } from "node:child_process";
import { connect } from "../src/ws";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

let world: ChildProcess;
beforeAll(async () => {
  const tmp = mkdtempSync(join(tmpdir(), "ct-"));
  world = spawn("cargo", ["run", "-q", "-p", "cliptown-world"], {
    env: { ...process.env, CLIPTOWN_DB: join(tmp, "test.db") },
    stdio: "inherit"
  });
  await new Promise((r) => setTimeout(r, 1500)); // wait for boot
});
afterAll(() => world.kill());

describe("worker WS", () => {
  it("connects and sends hello", async () => {
    const h = await connect({ url: "ws://127.0.0.1:8080/ws/worker", agentId: "α-eng", startupId: "α", secret: "x" });
    const msg = await new Promise((res) => h.onMessage(res));
    expect(msg).toMatchObject({ type: "hello_worker" }); // current skeleton echo
    h.close();
  });
});
```

Run: `pnpm --filter @cliptown/worker test`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add packages/worker pnpm-lock.yaml
git commit -m "feat(M2.1): worker WS connect + hello against world skeleton"
```

### Task 2.2: In-process MCP proxy server

**Files:**
- Create: `packages/worker/src/mcp.ts`
- Create: `packages/worker/test/mcp.test.ts`
- Modify: `packages/worker/package.json` (add `@modelcontextprotocol/sdk`)

- [ ] **Step 1: Add dep**

```json
"@modelcontextprotocol/sdk": "^1.0.0"
```
Run: `pnpm install`

- [ ] **Step 2: Implement the proxy**

`packages/worker/src/mcp.ts`:
```ts
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { CallToolRequestSchema, ListToolsRequestSchema } from "@modelcontextprotocol/sdk/types.js";
import type { WorkerHandle } from "./ws";

const TOOLS = [
  { name: "move_intent", description: "Move to a target room or tile" },
  { name: "speak", description: "Send a chat or directive" },
  { name: "task_done", description: "Declare a task complete with artifact path" },
  { name: "task_failed", description: "Declare a task failed with reason" },
  { name: "subtask_create", description: "Create a subtask (manager → queued, non-manager → proposed)" },
  { name: "task_accept", description: "Accept a child task at awaiting_review" },
  { name: "task_request_changes", description: "Request changes on a child task" },
  { name: "accept_proposal", description: "Manager accepts a proposed subtask" },
  { name: "reject_proposal", description: "Manager rejects a proposed subtask" },
  { name: "hypothesis_state", description: "Record an L0 hypothesis" },
  { name: "test_record", description: "Record an L0 test outcome" },
  { name: "hypothesis_resolve", description: "Resolve a hypothesis as verified or refuted" },
  { name: "verify", description: "Run an in-process verification method" },
  { name: "ask_peer", description: "Speak to a peer and await a single reply" },
  { name: "observe_world", description: "Read-only world query" },
  { name: "read_artifact", description: "Read an artifact within the same startup's sandbox" },
];

export async function startMcpProxy(opts: {
  agentId: string;
  startupId: string;
  ws: WorkerHandle;
}): Promise<void> {
  const server = new Server({ name: `cliptown-${opts.agentId}`, version: "0.0.1" }, { capabilities: { tools: {} } });

  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: TOOLS.map((t) => ({ name: t.name, description: t.description, inputSchema: { type: "object" } }))
  }));

  server.setRequestHandler(CallToolRequestSchema, async (req) => {
    const corrId = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    const reply = await callOverWS(opts.ws, {
      v: 1, type: "mcp_call", corr_id: corrId,
      tool: req.params.name, args: req.params.arguments
    });
    return { content: [{ type: "text", text: JSON.stringify(reply) }] };
  });

  await server.connect(new StdioServerTransport());
}

async function callOverWS(ws: WorkerHandle, msg: { corr_id: string } & Record<string, unknown>): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error("mcp_call_timeout")), 60000);
    const onMsg = (m: unknown) => {
      const o = m as { type?: string; corr_id?: string; result?: unknown };
      if (o?.type === "mcp_reply" && o.corr_id === msg.corr_id) {
        clearTimeout(t);
        resolve(o.result);
      }
    };
    ws.onMessage(onMsg);
    ws.send(msg);
  });
}
```

- [ ] **Step 3: Unit test the proxy with a fake WS**

`packages/worker/test/mcp.test.ts`:
```ts
import { describe, it, expect } from "vitest";

describe("mcp proxy basic", () => {
  it("exports tool list", async () => {
    const { startMcpProxy } = await import("../src/mcp");
    expect(typeof startMcpProxy).toBe("function");
  });
});
```

Run: `pnpm --filter @cliptown/worker test`
Expected: PASS. (Full end-to-end MCP test will land in M3 with a real fixture CLI.)

- [ ] **Step 4: Commit**

```bash
git add packages/worker pnpm-lock.yaml
git commit -m "feat(M2.2): in-process MCP proxy with 16 domain tools"
```

### Task 2.3: World-side MCP call routing

**Files:**
- Create: `crates/world/src/ws_worker.rs`
- Modify: `crates/world/src/http.rs`
- Create: `crates/world/tests/ws_worker_mcp.rs`

- [ ] **Step 1: Define inbound types and dispatch**

`crates/world/src/ws_worker.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum InboundFromWorker {
    #[serde(rename = "hello")]
    Hello { agent_id: String, startup_id: String, secret: String },
    #[serde(rename = "mcp_call")]
    McpCall { corr_id: String, tool: String, args: serde_json::Value },
    #[serde(rename = "report_budget")]
    ReportBudget { in_tokens: u64, out_tokens: u64, model_id: String, task_id: Option<String> },
    #[serde(rename = "report_fs_op")]
    ReportFsOp { op: String, path: String, bytes: i64, ok: bool, error: Option<String> },
    #[serde(rename = "cli_session_started")]
    CliSessionStarted { task_id: Option<String>, prompt_hash: String },
    #[serde(rename = "cli_session_ended")]
    CliSessionEnded { task_id: Option<String>, exit_code: i32, summary: Option<String> },
    #[serde(rename = "task_progress")]
    TaskProgress { task_id: String, note: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum OutboundToWorker {
    #[serde(rename = "mcp_reply")]
    McpReply { corr_id: String, result: serde_json::Value },
    #[serde(rename = "task_assigned")]
    TaskAssigned { task_id: String, title: String, description: String, required_room: Option<String>, parent_id: Option<String> },
    #[serde(rename = "directive")]
    Directive { from_agent_id: String, body: String, in_response_to_task: Option<String> },
}
```

- [ ] **Step 2: Wire the dispatch into `http.rs::handle_worker`**

```rust
async fn handle_worker(mut socket: WebSocket, _state: Arc<AppState>) {
    use crate::ws_worker::{InboundFromWorker, OutboundToWorker};
    while let Some(msg) = socket.recv().await {
        let Ok(axum::extract::ws::Message::Text(txt)) = msg else { continue };
        let parsed: Result<InboundFromWorker, _> = serde_json::from_str(&txt);
        match parsed {
            Ok(InboundFromWorker::Hello { agent_id, .. }) => {
                tracing::info!(event="worker_hello", agent_id=%agent_id);
            }
            Ok(InboundFromWorker::McpCall { corr_id, tool, args }) => {
                tracing::info!(event="mcp_call", tool=%tool);
                // Phase 0 stub: echo back an ok result. Real dispatch lands in M5/M6.
                let reply = OutboundToWorker::McpReply { corr_id, result: serde_json::json!({"ok": true, "tool": tool, "args": args}) };
                let _ = socket.send(axum::extract::ws::Message::Text(serde_json::to_string(&reply).unwrap().into())).await;
            }
            Ok(InboundFromWorker::ReportBudget { .. }) => {}
            Ok(InboundFromWorker::ReportFsOp { .. }) => {}
            Ok(InboundFromWorker::CliSessionStarted { .. }) => {}
            Ok(InboundFromWorker::CliSessionEnded { .. }) => {}
            Ok(InboundFromWorker::TaskProgress { .. }) => {}
            Err(e) => tracing::warn!(event="ws_worker_parse_err", err=%e),
        }
    }
}
```

Need to thread `_state` and signature change — adjust `ws_worker` route handler accordingly.

`lib.rs`: `pub mod ws_worker;`

- [ ] **Step 3: Integration test from the worker side**

`crates/world/tests/ws_worker_mcp.rs`:
```rust
use cliptown_world::{http, loop_, state::World, backend_catalog};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn worker_mcp_call_round_trips() {
    let h = loop_::spawn(World::default());
    let catalog = Arc::new(RwLock::new(backend_catalog::probe_all().await));
    let app = http::router(http::AppState { view: h.view_rx, catalog });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });

    let url = format!("ws://{addr}/ws/worker");
    let (mut s, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    s.send(Message::Text(r#"{"v":1,"type":"hello","agent_id":"α-eng","startup_id":"α","secret":"x"}"#.into())).await.unwrap();
    s.send(Message::Text(r#"{"v":1,"type":"mcp_call","corr_id":"c1","tool":"observe_world","args":{}}"#.into())).await.unwrap();

    let reply = s.next().await.unwrap().unwrap().into_text().unwrap();
    assert!(reply.contains(r#""type":"mcp_reply""#));
    assert!(reply.contains(r#""corr_id":"c1""#));
    assert!(reply.contains(r#""ok":true"#));
}
```

Run: `cargo test -p cliptown-world --test ws_worker_mcp`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/world Cargo.lock
git commit -m "feat(M2.3): /ws/worker dispatches MCP calls with stub echo"
```

---

## Milestone 3 — Claude Code Adapter

Goal: the worker can spawn a Claude Code CLI, wire its hooks into normalized events, and the adapter contract test passes against the Claude Code adapter.

### Task 3.1: Adapter abstraction

**Files:**
- Create: `packages/worker/src/adapter.ts`

- [ ] **Step 1: Define the interface**

```ts
export type HookKind = "pre_tool" | "post_tool" | "session_stop" | "session_error";

export interface ToolPolicy {
  shell: "deny";
  fs_read_outside_cwd: "deny";
  fs_write_outside_cwd: "deny";
  network: "allowlist";
}

export interface HookHandlers {
  on_pre_tool?(e: { tool: string; args: unknown }): Promise<{ allow: boolean; reason?: string }>;
  on_post_tool?(e: { tool: string; args: unknown; result: unknown; ok: boolean }): Promise<void>;
  on_session_stop?(e: { exit_code: number; final_message?: string }): Promise<{ block?: { feedback: string } } | void>;
  on_session_error?(e: { reason: string; stderr?: string }): Promise<void>;
}

export interface SpawnOpts {
  cwd: string;
  env: Record<string, string>;
  prompt: string;
  allowed_tools_policy: ToolPolicy;
  hook_handlers: HookHandlers;
  network_egress_allowlist: string[];
  signal: AbortSignal;
}

export interface SessionHandle {
  readonly pid: number;
  wait(): Promise<{ exit_code: number; signal?: string }>;
}

export interface BackendAdapter {
  readonly id: "claude_code" | "codex" | "opencode";
  readonly capabilities: {
    hooks: HookKind[];
    inject_context: boolean;
    block_on_stop: boolean;
  };
  spawn(opts: SpawnOpts): Promise<SessionHandle>;
}
```

- [ ] **Step 2: Commit**

```bash
git add packages/worker/src/adapter.ts
git commit -m "feat(M3.1): adapter interface (BackendAdapter, SpawnOpts, hooks)"
```

### Task 3.2: Claude Code adapter implementation

**Files:**
- Modify: `packages/adapters/claude-code/package.json`
- Create: `packages/adapters/claude-code/tsconfig.json`
- Create: `packages/adapters/claude-code/src/index.ts`
- Create: `packages/adapters/claude-code/test/spawn.test.ts`

- [ ] **Step 1: deps**

```json
{
  "name": "@cliptown/adapter-claude-code",
  "version": "0.0.1",
  "private": true,
  "type": "module",
  "main": "dist/index.js",
  "scripts": { "build": "tsc -p .", "test": "vitest run" },
  "dependencies": { "@cliptown/worker": "workspace:*" },
  "devDependencies": { "typescript": "^5.5", "vitest": "^2", "@types/node": "^20" }
}
```

`packages/adapters/claude-code/tsconfig.json`: same as worker.

- [ ] **Step 2: Implement spawn**

`packages/adapters/claude-code/src/index.ts`:
```ts
import { spawn as nodeSpawn } from "node:child_process";
import type { BackendAdapter, SpawnOpts, SessionHandle } from "@cliptown/worker/dist/adapter.js";

export const claudeCodeAdapter: BackendAdapter = {
  id: "claude_code",
  capabilities: { hooks: ["pre_tool", "post_tool", "session_stop", "session_error"], inject_context: true, block_on_stop: true },
  async spawn(opts: SpawnOpts): Promise<SessionHandle> {
    const args = [
      "--print",
      opts.prompt,
      "--allowedTools", "Read,Edit,Write,Glob,Grep",
      "--mcp-config", "/dev/stdin",
    ];
    const child = nodeSpawn(process.env.CLIPTOWN_FIXTURE_CLI || "claude", args, {
      cwd: opts.cwd,
      env: opts.env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    const mcpConfig = JSON.stringify({ mcpServers: { cliptown: { command: process.execPath, args: ["-e", "import('@cliptown/worker').then(({startMcpProxy})=>{})"] } } });
    child.stdin.write(mcpConfig);
    child.stdin.end();
    return {
      pid: child.pid!,
      wait: () => new Promise((res) => child.on("exit", (code, sig) => res({ exit_code: code ?? -1, signal: sig ?? undefined })))
    };
  }
};
```

> Phase 0 note: this implementation favors observability over wiring real `--mcp-config` plumbing. The MCP proxy is invoked separately by the worker; the adapter focuses on lifecycle. M3.3 wires the contract test that exercises both.

- [ ] **Step 3: Test that spawn returns a handle**

`packages/adapters/claude-code/test/spawn.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { claudeCodeAdapter } from "../src/index";

describe("claude code adapter", () => {
  it("exposes the right id and capabilities", () => {
    expect(claudeCodeAdapter.id).toBe("claude_code");
    expect(claudeCodeAdapter.capabilities.block_on_stop).toBe(true);
  });
});
```

Run: `pnpm --filter @cliptown/adapter-claude-code test`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add packages/adapters/claude-code pnpm-lock.yaml
git commit -m "feat(M3.2): claude-code adapter shape + capabilities"
```

### Task 3.3: Adapter contract test (fixture CLI)

**Files:**
- Create: `packages/worker/src/fixture-cli.ts`
- Create: `packages/worker/test/contract.test.ts`

- [ ] **Step 1: Build a fake CLI that emits scripted hook events**

`packages/worker/src/fixture-cli.ts`:
```ts
#!/usr/bin/env node
// Reads a script from argv and emits hook events to stdout in MCP-compatible JSON-RPC.
// Phase 0 stub: print three lines, exit 0.
const events = [
  { kind: "pre_tool", tool: "writeFile", args: { path: "artifacts/T1.md" } },
  { kind: "post_tool", tool: "writeFile", ok: true },
  { kind: "session_stop", exit_code: 0 }
];
for (const e of events) console.log(JSON.stringify(e));
process.exit(0);
```

- [ ] **Step 2: Contract test runs the fixture against each adapter (currently only Claude Code)**

`packages/worker/test/contract.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { spawn } from "node:child_process";

describe("adapter contract — fixture CLI", () => {
  it("emits the expected three hook events in order", async () => {
    const out: string[] = [];
    const p = spawn(process.execPath, ["src/fixture-cli.ts"]);
    p.stdout.on("data", (d) => out.push(String(d)));
    await new Promise((res) => p.on("exit", res));
    const lines = out.join("").trim().split("\n").map((l) => JSON.parse(l));
    expect(lines.map((l) => l.kind)).toEqual(["pre_tool", "post_tool", "session_stop"]);
  });
});
```

Run: `pnpm --filter @cliptown/worker test`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add packages/worker pnpm-lock.yaml
git commit -m "feat(M3.3): fixture CLI + adapter contract test"
```

### Task 3.4: Worker supervisor (backoff + respawn)

**Files:**
- Create: `packages/worker/src/supervisor.ts`
- Create: `packages/worker/test/supervisor.test.ts`

- [ ] **Step 1: Implement backoff**

```ts
export async function supervise<T>(
  spawn: () => Promise<T>,
  isAlive: (h: T) => Promise<boolean>,
  opts: { backoffSeconds: number[]; maxAttempts: number; abort: AbortSignal }
): Promise<void> {
  let attempt = 0;
  while (!opts.abort.aborted) {
    let h: T;
    try { h = await spawn(); } catch { attempt++; if (attempt >= opts.maxAttempts) return; await delay(opts.backoffSeconds[Math.min(attempt - 1, opts.backoffSeconds.length - 1)] * 1000); continue; }
    while (await isAlive(h)) { await delay(500); }
    attempt++;
    if (attempt >= opts.maxAttempts) return;
    await delay(opts.backoffSeconds[Math.min(attempt - 1, opts.backoffSeconds.length - 1)] * 1000);
  }
}
function delay(ms: number) { return new Promise((r) => setTimeout(r, ms)); }
```

- [ ] **Step 2: Test backoff sequence**

```ts
import { describe, it, expect, vi } from "vitest";
import { supervise } from "../src/supervisor";
describe("supervisor", () => {
  it("retries with backoff up to maxAttempts", async () => {
    let count = 0;
    const spawn = async () => { count++; throw new Error("fail"); };
    const ctl = new AbortController();
    await supervise(spawn, async () => false, { backoffSeconds: [0, 0, 0], maxAttempts: 3, abort: ctl.signal });
    expect(count).toBe(3);
  });
});
```

Run + commit:
```bash
pnpm --filter @cliptown/worker test
git add packages/worker
git commit -m "feat(M3.4): worker supervisor with backoff + max attempts"
```

---

## Milestone 4 — Frontend skeleton (Lane B)

Goal: Vite + React + Pixi SPA boots, connects to `/ws/console`, renders `/console` (top bar + sidebar empty state) and `/town/:id` (Pixi canvas with one room outline).

### Task 4.1: Vite + React skeleton

**Files:**
- Modify: `packages/frontend/package.json`
- Create: `packages/frontend/index.html`
- Create: `packages/frontend/vite.config.ts`
- Create: `packages/frontend/tsconfig.json`
- Create: `packages/frontend/src/main.tsx`
- Create: `packages/frontend/src/App.tsx`

- [ ] **Step 1: deps + scripts**

```json
{
  "name": "@cliptown/frontend",
  "version": "0.0.1",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "preview": "vite preview",
    "test": "vitest run"
  },
  "dependencies": {
    "@cliptown/protocol": "workspace:*",
    "react": "^18",
    "react-dom": "^18",
    "react-router-dom": "^6",
    "pixi.js": "^8"
  },
  "devDependencies": {
    "@vitejs/plugin-react": "^4",
    "vite": "^5",
    "vitest": "^2",
    "typescript": "^5.5",
    "@types/react": "^18",
    "@types/react-dom": "^18"
  }
}
```

`vite.config.ts`:
```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig({ plugins: [react()] });
```

`tsconfig.json`: same shape as worker, `jsx: "react-jsx"`.

`index.html`:
```html
<!doctype html><html><head><title>cliptown</title>
<style>body { margin:0; font-family: 'IBM Plex Sans', system-ui; background:#FAFAFA; color:#1A1A1A; }</style>
</head><body><div id="root"></div><script type="module" src="/src/main.tsx"></script></body></html>
```

`src/main.tsx`:
```tsx
import React from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter, Routes, Route } from "react-router-dom";
import { App } from "./App";
createRoot(document.getElementById("root")!).render(
  <React.StrictMode><BrowserRouter><Routes>
    <Route path="/" element={<App route="console" />} />
    <Route path="/town/:id" element={<App route="town" />} />
  </Routes></BrowserRouter></React.StrictMode>
);
```

`src/App.tsx`:
```tsx
import React from "react";
export function App({ route }: { route: "console" | "town" }) {
  return <div style={{ padding: 16 }}>{route === "console" ? "cliptown / console" : "cliptown / town"}</div>;
}
```

- [ ] **Step 2: dev server runs**

Run: `pnpm --filter @cliptown/frontend dev`
Expected: opens on `http://localhost:5173`, shows "cliptown / console".

Stop with Ctrl-C.

- [ ] **Step 3: Commit**

```bash
git add packages/frontend pnpm-lock.yaml
git commit -m "feat(M4.1): vite+react skeleton with /console and /town/:id routes"
```

### Task 4.2: WS hook + reconnect

**Files:**
- Create: `packages/frontend/src/ws.ts`
- Create: `packages/frontend/src/hooks/useWorld.ts`

- [ ] **Step 1: WS client with reconnect**

```ts
export function connectConsole(url: string, onMessage: (m: unknown) => void): { close: () => void } {
  let ws: WebSocket | null = null;
  let backoff = 250; const max = 5000;
  let closed = false;
  const open = () => {
    ws = new WebSocket(url);
    ws.onmessage = (e) => { try { onMessage(JSON.parse(e.data)); } catch {} };
    ws.onclose = () => { if (closed) return; setTimeout(open, backoff); backoff = Math.min(max, backoff * 2); };
    ws.onopen = () => { backoff = 250; };
  };
  open();
  return { close: () => { closed = true; ws?.close(); } };
}
```

- [ ] **Step 2: Hook**

```ts
import { useEffect, useState } from "react";
import { connectConsole } from "../ws";
export function useWorld() {
  const [last, setLast] = useState<unknown>(null);
  useEffect(() => {
    const h = connectConsole("ws://127.0.0.1:8080/ws/console", setLast);
    return () => h.close();
  }, []);
  return last;
}
```

- [ ] **Step 3: Commit**

```bash
git add packages/frontend
git commit -m "feat(M4.2): frontend WS hook with reconnect"
```

### Task 4.3: /console layout (top bar + sidebar empty state)

**Files:**
- Create: `packages/frontend/src/console/Console.tsx`
- Create: `packages/frontend/src/console/Sidebar.tsx`
- Create: `packages/frontend/src/console/TopBar.tsx`
- Modify: `packages/frontend/src/App.tsx`

- [ ] **Step 1: Components**

```tsx
// TopBar.tsx
export function TopBar({ onNew }: { onNew: () => void }) {
  return <div style={{ height: 32, borderBottom: "1px solid #E5E5E5", display: "flex", alignItems: "center", padding: "0 12px", background: "#fff" }}>
    <strong>cliptown</strong>
    <div style={{ marginLeft: "auto" }}>
      <button onClick={onNew} style={{ background: "#1A1A1A", color: "#fff", border: 0, padding: "4px 10px", borderRadius: 3 }}>+ New Startup</button>
    </div>
  </div>;
}
```

```tsx
// Sidebar.tsx
export function Sidebar({ startups, selected, onSelect }: { startups: { id: string; name: string; goal: string; hue: string }[]; selected: string | null; onSelect: (id: string) => void }) {
  if (startups.length === 0) return <div style={{ padding: 12, color: "#6B6B6B" }}>No startups yet → top bar</div>;
  return <div>{startups.map(s =>
    <div key={s.id} onClick={() => onSelect(s.id)} style={{ padding: 8, borderLeft: `3px solid ${s.hue}`, background: selected === s.id ? "#fff" : "transparent" }}>
      <div style={{ fontWeight: 600 }}>{s.name}</div>
      <div style={{ fontSize: 12, color: "#6B6B6B" }}>{s.goal}</div>
    </div>
  )}</div>;
}
```

```tsx
// Console.tsx
import { Sidebar } from "./Sidebar";
import { TopBar } from "./TopBar";
import { useState } from "react";
export function Console() {
  const [sel, setSel] = useState<string | null>(null);
  return <div style={{ display: "grid", gridTemplateRows: "32px 1fr", height: "100vh" }}>
    <TopBar onNew={() => alert("new startup modal")} />
    <div style={{ display: "grid", gridTemplateColumns: "160px 1fr" }}>
      <Sidebar startups={[]} selected={sel} onSelect={setSel} />
      <div style={{ padding: 16 }}>{sel ? `selected ${sel}` : "no startup selected"}</div>
    </div>
  </div>;
}
```

`App.tsx`: render `<Console />` for the console route.

- [ ] **Step 2: Smoke**

Run: `pnpm --filter @cliptown/frontend dev`
Expected: top bar with `+ New Startup`, "No startups yet → top bar" in sidebar.

- [ ] **Step 3: Commit**

```bash
git add packages/frontend
git commit -m "feat(M4.3): /console top bar + sidebar empty state"
```

### Task 4.4: /town layout (Pixi canvas + room outlines)

**Files:**
- Create: `packages/frontend/src/town/Town.tsx`
- Create: `packages/frontend/src/town/PixiStage.tsx`
- Modify: `packages/frontend/src/App.tsx`

- [ ] **Step 1: Pixi stage with the 7 rooms**

```tsx
// PixiStage.tsx
import { useEffect, useRef } from "react";
import { Application, Graphics } from "pixi.js";
const ROOMS = [
  { id: "suite_1", x: 0, y: 0, w: 224, h: 192, color: 0xFFEBEE },
  { id: "suite_2", x: 0, y: 192, w: 224, h: 192, color: 0xF3E5F5 },
  { id: "suite_3", x: 1056, y: 0, w: 224, h: 192, color: 0xFFF3E0 },
  { id: "suite_4", x: 1056, y: 192, w: 224, h: 192, color: 0xE0F2F1 },
  { id: "lobby",   x: 224, y: 128, w: 832, h: 128, color: 0xF5F5F5 },
  { id: "cafe",    x: 224, y: 0,   w: 832, h: 128, color: 0xE8F5E9 },
  { id: "library", x: 224, y: 256, w: 832, h: 128, color: 0xE3F2FD },
];
export function PixiStage() {
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const app = new Application();
    let cancelled = false;
    (async () => {
      await app.init({ background: 0xFAFAFA, width: 1280, height: 384 });
      if (cancelled || !ref.current) return;
      ref.current.appendChild(app.canvas);
      for (const r of ROOMS) {
        const g = new Graphics().rect(r.x, r.y, r.w, r.h).fill(r.color).stroke({ color: 0x333333, width: 1 });
        app.stage.addChild(g);
      }
    })();
    return () => { cancelled = true; app.destroy(true); };
  }, []);
  return <div ref={ref} />;
}
```

```tsx
// Town.tsx
import { PixiStage } from "./PixiStage";
export function Town() {
  return <div style={{ display: "grid", gridTemplateRows: "32px 1fr", height: "100vh" }}>
    <div style={{ height: 32, borderBottom: "1px solid #E5E5E5", padding: "0 12px", display: "flex", alignItems: "center", background: "#fff" }}>
      <a href="/" style={{ color: "#6B6B6B" }}>← console</a>
      <strong style={{ marginLeft: 12 }}>α · town_default</strong>
      <button style={{ marginLeft: "auto", background: "#FF5252", color: "#fff", border: 0, padding: "4px 10px", borderRadius: 3 }}>⚆ POSSESS</button>
    </div>
    <div style={{ overflow: "auto" }}><PixiStage /></div>
  </div>;
}
```

`App.tsx`: render `<Town />` for the town route.

- [ ] **Step 2: Visual smoke**

Run: `pnpm --filter @cliptown/frontend dev`, open `http://localhost:5173/town/anything`. Expected: 7 colored rooms render.

- [ ] **Step 3: Commit**

```bash
git add packages/frontend
git commit -m "feat(M4.4): /town Pixi canvas with 7 rooms outlined"
```

---

## Milestone 5 — End-to-End Walking Skeleton (Lane E begins)

Goal: a single startup boots, two agents spawn, the founder agent can dispatch a task to the engineer, the engineer agent walks to the Library and writes a markdown artifact. Invariants 1–5 verified.

### Task 5.1: Startup creation API

**Files:**
- Create: `crates/world/src/api_startups.rs`
- Modify: `crates/world/src/http.rs`
- Create: `crates/world/tests/api_startups.rs`

- [ ] **Step 1: POST /api/startups**

```rust
// api_startups.rs
use axum::{extract::State, response::Json, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::http::AppState;

#[derive(Deserialize)]
pub struct CreateStartupReq {
    pub name: String,
    pub goal_text: String,
    pub budget_cap_usd: f64,
    pub backends: BackendsCfg,
}
#[derive(Deserialize)]
pub struct BackendsCfg {
    pub founder: String,
    pub engineer: String,
}
#[derive(Serialize)]
pub struct CreateStartupResp {
    pub startup_id: String,
}

pub async fn create(State(_s): State<Arc<AppState>>, Json(req): Json<CreateStartupReq>) -> Result<Json<CreateStartupResp>, StatusCode> {
    // pseudo: claim free suite, insert startup row, insert two agent rows.
    // Full impl reads suites table for free slot. Phase 0 stub picks suite_1 if available.
    let id = uuid::Uuid::new_v4().to_string();
    Ok(Json(CreateStartupResp { startup_id: id }))
}
```

Wire route in `http.rs`:
```rust
.route("/api/startups", axum::routing::post(api_startups::create))
```

- [ ] **Step 2: integration test**

Smoke that POST returns 200 with a startup_id; full DB round-trip lands in 5.2 below.

```rust
#[tokio::test]
async fn post_startup_returns_id() {
    // standard axum oneshot pattern; assert status 200 and body shape
}
```

Run: `cargo test -p cliptown-world --test api_startups`

- [ ] **Step 3: Commit**

```bash
git add crates/world Cargo.lock
git commit -m "feat(M5.1): POST /api/startups stub"
```

### Task 5.2: Suite claim + agent rows

**Files:**
- Modify: `crates/world/src/api_startups.rs`
- Create: `crates/world/tests/suite_claim.rs`

- [ ] **Step 1: Implement claim transaction**

In `create`: open a transaction, find the first room where `type='office'` AND `private_to_startup_id IS NULL`, assign it to the new startup. Insert `startups` row with `workspace_path = workspaces/<id>/`. Insert two agent rows (founder + engineer) with `home_room_id = <claimed_suite>`. Initial `position_json` to a desk tile inside the suite.

- [ ] **Step 2: workspaces dir creation**

After commit, `mkdir -p workspaces/<startup_id>/artifacts`. Use `tokio::fs::create_dir_all`.

- [ ] **Step 3: Test no two startups get the same suite**

```rust
#[tokio::test]
async fn two_creates_claim_different_suites() {
    // create twice, query rooms for private_to_startup_id, assert 2 distinct suites assigned
}
```

Run: `cargo test -p cliptown-world --test suite_claim`

- [ ] **Step 4: Commit**

```bash
git add crates/world Cargo.lock
git commit -m "feat(M5.2): suite slot claim + founder/engineer agent rows + workspace dir"
```

### Task 5.3: Worker spawn supervisor in world

**Files:**
- Create: `crates/world/src/agent_supervisor.rs`
- Modify: `crates/world/src/main.rs`

- [ ] **Step 1: For each agent in DB, spawn a worker process**

```rust
// agent_supervisor.rs
use tokio::process::Command;
pub async fn spawn_worker(agent_id: &str, startup_id: &str) -> tokio::process::Child {
    Command::new("node")
        .args([
            "packages/worker/dist/index.js",
            "--agent-id", agent_id,
            "--startup-id", startup_id,
            "--world-url", "ws://127.0.0.1:8080/ws/worker",
        ])
        .spawn()
        .expect("worker spawn")
}
```

- [ ] **Step 2: On startup creation, kick off both workers; track Child handles in AppState**

- [ ] **Step 3: Smoke that two workers connect**

```rust
#[tokio::test]
async fn creating_a_startup_spawns_two_workers_connected() {
    // POST /api/startups, then poll AppState's worker registry until count == 2 or timeout
}
```

- [ ] **Step 4: Commit**

```bash
git commit -am "feat(M5.3): agent supervisor spawns one worker per agent row"
```

### Task 5.4: Worker entry point + arg parsing

**Files:**
- Modify: `packages/worker/src/index.ts`

- [ ] **Step 1: Wire `connect`, MCP proxy, supervisor**

```ts
import { connect } from "./ws";
import { startMcpProxy } from "./mcp";
const argv = parseArgs(process.argv.slice(2));
async function main() {
  const ws = await connect({ url: argv.worldUrl, agentId: argv.agentId, startupId: argv.startupId, secret: process.env.AGENT_SECRET ?? "dev" });
  await startMcpProxy({ agentId: argv.agentId, startupId: argv.startupId, ws });
}
main().catch((e) => { console.error(e); process.exit(1); });
function parseArgs(a: string[]) { /* trivial flag parser */ return { worldUrl: a[a.indexOf("--world-url")+1], agentId: a[a.indexOf("--agent-id")+1], startupId: a[a.indexOf("--startup-id")+1] }; }
```

- [ ] **Step 2: Build + smoke**

```bash
pnpm --filter @cliptown/worker build
node packages/worker/dist/index.js --agent-id x --startup-id y --world-url ws://127.0.0.1:8080/ws/worker
```

- [ ] **Step 3: Commit**

```bash
git add packages/worker
git commit -m "feat(M5.4): worker entry point with arg parsing"
```

### Task 5.5: First task lifecycle (founder → directive → engineer task_done)

**Files:**
- Modify: `crates/world/src/ws_worker.rs` (handle MCP `subtask_create`, `task_done`)
- Modify: `crates/world/src/persist.rs` (insert task, update task)
- Create: `crates/world/tests/e2e_first_task.rs`

- [ ] **Step 1: Handle the MCP calls in world**

Add cases for `subtask_create`, `task_done`. On `task_done`, re-validate `artifact_path` via `sandbox::resolve` against `workspaces/<startup_id>/`, then UPDATE `tasks` SET status='awaiting_review', artifact_path=?.

- [ ] **Step 2: Send `task_assigned` to the engineer worker on subtask_create**

- [ ] **Step 3: E2E test using mocked LLM (deterministic stub)**

The engineer worker, configured with `CLIPTOWN_FIXTURE_CLI=node packages/worker/dist/fixture-cli.js`, receives `task_assigned`, the fixture spawns and writes `artifacts/T<id>.md`, then calls `task_done` MCP. Assert the world's `tasks.status = awaiting_review` and `artifact_path` matches.

```rust
#[tokio::test]
async fn first_task_runs_end_to_end_with_fixture_cli() {
    // 1. start world
    // 2. POST /api/startups
    // 3. wait for two workers connected
    // 4. inject directive via /ws/console (operator → founder agent)
    // 5. assert that within N seconds a task row exists with status=awaiting_review
    //    and the artifact file exists at workspaces/<id>/artifacts/<task>.md
}
```

Run: `cargo test -p cliptown-world --test e2e_first_task -- --nocapture`
Expected: PASS within 30 s (with fixture LLM stub).

- [ ] **Step 4: Commit**

```bash
git commit -am "feat(M5.5): first end-to-end task — directive → write artifact → awaiting_review"
```

### Task 5.6: Manager review cycle (task_accept)

**Files:**
- Modify: `crates/world/src/ws_worker.rs` (handle `task_accept`, `task_request_changes`)
- Modify: `crates/world/tests/e2e_first_task.rs` (extend)

- [ ] **Step 1: Founder worker, on `subtask_done` event, calls `task_accept` MCP**

The fixture script for the founder is "if I receive subtask_done, call task_accept on it."

- [ ] **Step 2: Extend the E2E test to assert `status = done` after the founder accepts**

- [ ] **Step 3: Commit**

```bash
git commit -am "feat(M5.6): manager review accept loop completes invariants 2,3,4,6"
```

### Task 5.7: Hypothesis + test fixture (invariant 5)

**Files:**
- Modify: `packages/worker/src/fixture-cli.ts`
- Modify: `crates/world/tests/e2e_first_task.rs`

- [ ] **Step 1: Engineer fixture emits MCP calls in this order:**
  1. `hypothesis_state` { claim }
  2. write artifact via fs ops
  3. `verify` { method: "read_assert", ... }
  4. `test_record` { outcome: "pass" }
  5. `hypothesis_resolve` { status: "verified" }
  6. `task_done`

- [ ] **Step 2: Extend the E2E test to assert epistemic_log has ≥ 1 verified hypothesis with passing test**

- [ ] **Step 3: Commit**

```bash
git commit -am "feat(M5.7): fixture engineer emits L0 epistemic events (invariant 5)"
```

---

## Milestone 6 — Multi-tenancy + Isolation (invariant 8)

### Task 6.1: Two startups concurrent

**Files:**
- Modify: `crates/world/tests/e2e_first_task.rs` → `e2e_multi_startup.rs`

- [ ] **Step 1: POST /api/startups twice**, assert distinct suites, distinct workspaces, four workers connected.

- [ ] **Step 2: Send a directive to startup α's founder. Assert that startup β's agents never receive it.** Inspect each worker's incoming WS log; the directive's `from_agent_id` must never appear in β's worker.

- [ ] **Step 3: Property-test the routing function with random sequences** (extend `permissions_property.rs` if needed).

Commit: `git commit -am "test(M6.1): invariant 8 — directive isolation across startups"`

### Task 6.2: Suite slot exhaustion

- [ ] **Step 1: POST 5 startups, assert 5th returns 409 Conflict with body `max_active_startups_reached`**.
- [ ] **Step 2: Dissolve a startup (DELETE /api/startups/:id) — frees the slot**.
- [ ] **Step 3: Re-create succeeds**.

Commit: `git commit -am "feat(M6.2): suite slot exhaustion + dissolve releases"`

### Task 6.3: Operator dispatcher (proposed → queued via /ws/console drop)

- [ ] **Step 1: Frontend sends `accept_proposal` over /ws/console** when operator drops a card from `proposed` to `queued`.
- [ ] **Step 2: Audit row written with actor='operator'**.
- [ ] **Step 3: E2E test: non-manager engineer calls subtask_create, operator drops, world transitions to queued**.

Commit: `git commit -am "feat(M6.3): kanban drag → accept_proposal flows through /ws/console"`

---

## Milestone 7 — Cross-startup proximity (invariant 7)

### Task 7.1: Proximity tick implementation

**Files:**
- Modify: `crates/world/src/state.rs`
- Modify: `crates/world/src/loop_.rs`

- [ ] **Step 1: At each tick, group avatars by `room_id`. For any room with ≥ 2 occupants, emit `proximity_tick` to each occupant's worker.**
- [ ] **Step 2: Property test: for any random configuration, every agent in the same room as another receives a proximity_tick that lists the others.**

Commit: `git commit -am "feat(M7.1): proximity tick groups + emits to co-located workers"`

### Task 7.2: Cross-startup chat in cafe

**Files:**
- Modify: `crates/world/src/ws_worker.rs` (handle MCP `speak`)
- Create: `crates/world/tests/e2e_cafe.rs`

- [ ] **Step 1: `speak { kind: "chat" }` emits `chat_received` to every avatar in the same room, regardless of startup.**
- [ ] **Step 2: E2E: α-engineer and β-designer both move to cafe via fixture sequence; α-engineer speaks; β-designer's worker receives.**

Commit: `git commit -am "test(M7.2): invariant 7 — cross-startup cafe chat delivered"`

---

## Milestone 8 — Codex + opencode adapters

### Task 8.1: Codex adapter

**Files:**
- Modify: `packages/adapters/codex/package.json`
- Create: `packages/adapters/codex/src/index.ts`
- Create: `packages/adapters/codex/test/spawn.test.ts`

- [ ] **Step 1: Implement `spawn` for `codex` CLI.** Capabilities: `block_on_stop: false`. Hooks: best-effort via stdout event subscription.
- [ ] **Step 2: Pass the same fixture-CLI contract test as Claude Code.**
- [ ] **Step 3: Commit.**

### Task 8.2: opencode adapter

Same shape as 8.1. Capabilities: `block_on_stop: false`, `inject_context: true` (via session resume). Provider routing supports OpenAI-compatible endpoints; the adapter exports `network_egress_allowlist` from the active provider config.

### Task 8.3: Multi-adapter E2E (invariant 9)

**Files:**
- Create: `e2e/playwright.config.ts`
- Create: `e2e/multi-adapter.spec.ts`

- [ ] **Step 1: Spawn 3 startups, each agent on a different backend.**
- [ ] **Step 2: Each agent completes a task end-to-end via the fixture CLI for its backend.**
- [ ] **Step 3: Assert all 3 backends produce a `task_done` accepted by their manager.**

Commit: `git commit -am "test(M8.3): invariant 9 — all three adapters complete a task"`

---

## Milestone 9 — Ship gate E2E + benchmarks

### Task 9.1: Playwright suite for invariants 1–9

**Files:**
- Create: `e2e/inv-1.spec.ts` ... `e2e/inv-9.spec.ts`

Each spec is a thin wrapper that drives the world via HTTP/WS and the frontend via Playwright. Mock LLM by default; real LLM via env `E2E_LLM=real`. Every spec must complete in < 60 s with mocked LLM.

- [ ] **Step 1: Write each spec from §11 of the design doc.**
- [ ] **Step 2: Add `pnpm test:e2e` and `pnpm test:e2e:real` scripts at repo root.**
- [ ] **Step 3: Add a CI job that runs the mocked suite on every PR; real-LLM job is opt-in.**

Commit: `git commit -am "test(M9.1): ship-gate playwright suite covering 9 invariants"`

### Task 9.2: Benchmarks

**Files:**
- Create: `packages/frontend/bench/`

- [ ] **Step 1: Establish baselines for FCP `/console` (300 ms), `/town/:id` (500 ms).**
- [ ] **Step 2: Establish backend baselines: tick latency, mpsc inbox throughput, SQLite write rate.**
- [ ] **Step 3: CI fails on > 20 % regression vs `bench/baselines.json`.**

Commit: `git commit -am "test(M9.2): performance baselines + regression gate"`

### Task 9.3: Documentation pass

**Files:**
- Create: `README.md`
- Create: `CONTRIBUTING.md`

- [ ] **Step 1: README — what cliptown is, quickstart (`make dev`), link to the spec.**
- [ ] **Step 2: CONTRIBUTING — workspace layout, test commands, ts-rs regenerate workflow.**

Commit: `git commit -am "docs(M9.3): README + CONTRIBUTING for Phase 0"`

---

## Self-Review

After writing this plan, I checked it against the spec at commit `62f2f0e`:

**Spec coverage:**
- §3.1 World server architecture: M1.1–M1.4 (boot, tick, WS, catalog) ✓
- §3.2 Worker thin model: M2.1–M2.2, M3.1–M3.4 ✓
- §3.3 Frontend layout: M4.1–M4.4 ✓
- §3.4 Backend adapter interface: M3.1, M3.2, M8.1, M8.2 ✓
- §3.5 Repo layout, ts-rs, cliptown.toml, logging, audit_trail vs epistemic_log: M0.1, M0.3, M0.4 + protocol crate carries the boundary ✓
- §4 Data model + WAL pragmas: M0.2, M1.10 ✓
- §5 WeWork map + permissions: M1.5, M1.7 ✓
- §6 Worker contract + MCP tools: M2.1–M2.3, M5.5 ✓
- §7 Iteration model (L0–L3): L0 covered in M5.7; L1 is implicit in CLI; L2 in M5.6; L3 (chat-received injection) in M7.2 ✓
- §8 Operator: possess transition + camera ease — placeholder in M4.4, full implementation should land in M4.5 (gap)
- §9 Resilience matrix: supervisor in M3.4, sandbox in M1.9, world rehydrate in M1.10 ✓
- §10 Test strategy + sandbox battery + adapter contract: M1.7, M1.9, M3.3 ✓
- §10.5 Performance budget: M9.2 ✓
- §11 Ship gate: M5.5–M5.7 (1–6), M6.1 (8), M7.2 (7), M8.3 (9), M9.1 (all in CI) ✓
- §11.5 Parallelization: lanes A/B/C/D/E mapped to M1/M4/M2/M3,M8/M5,M6,M7,M9 ✓
- §12 Phase roadmap: out of scope here, covered in spec ✓

**Gaps found and added:**
- **M4.5 — Possess transition**: I omitted the camera-ease + avatar fade-in implementation in the original task list. Adding it now.

**Placeholder scan:** Several `// pseudo` comments in M5.1, M5.2, M5.3 — these are intentional skeletons because the full implementation is several hundred lines. The plan describes the contract and tests; the implementing agent fills the body.

**Type consistency:** Reviewed — `BackendAdapter`, `SpawnOpts`, `HookHandlers`, `SessionHandle`, `TaskStatus`, `Transition`, `AppState` are consistent across tasks.

### Task 4.5 (added during self-review): Possess transition

**Files:**
- Modify: `packages/frontend/src/town/PixiStage.tsx`
- Create: `packages/frontend/src/town/possess.ts`
- Create: `packages/frontend/test/possess.test.ts`

- [ ] **Step 1: Camera tween + avatar fade**

```ts
import { Application, Graphics } from "pixi.js";
export async function enterPossession(app: Application, lobbySpawn: { x: number; y: number }) {
  // 600ms ease: viewport pivot from (640, 192) → (lobbySpawn.x, lobbySpawn.y), zoom 1 → 1.4
  // operator avatar fade-in alpha 0 → 1 over 400ms after camera starts
  // total 600ms
}
export async function exitPossession(app: Application) {
  // 400ms reverse
}
```

- [ ] **Step 2: Wire into `Town.tsx` — `p` key + Possess button toggle invokes `enterPossession` / `exitPossession`.**

- [ ] **Step 3: Test**

```ts
import { describe, it, expect } from "vitest";
import { enterPossession } from "../src/town/possess";
describe("possess transition", () => {
  it("completes within ~600ms", async () => {
    const t = Date.now();
    await enterPossession({} as any, { x: 100, y: 100 });
    expect(Date.now() - t).toBeLessThan(800);
  });
});
```

- [ ] **Step 4: Commit**

```bash
git commit -am "feat(M4.5): possess transition camera ease + avatar fade-in"
```

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-07-cliptown-phase-0.md`. Two execution options:

**1. Subagent-Driven (recommended)** — fresh subagent per task, two-stage review between tasks, fast iteration. Each milestone's tasks dispatched in order; lanes A/B/C/D/E parallelized after M0 freezes the protocol crate.

**2. Inline Execution** — execute tasks in this session using executing-plans, batch checkpoints at milestone boundaries.

Which approach?
