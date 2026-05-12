# P2.1 daemon health buckets implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace cliptown's binary worker-liveness signal with a 4-state Health enum (`online` / `recently_lost` / `offline` / `about_to_gc`) derived from WS-connection state + `last_seen_at` so the operator console doesn't confuse a 5-minute network blip with a hard crash.

**Architecture:** New pure `crates/world/src/health.rs` module owns the derivation. `AvatarView` carries `last_seen_at: Option<i64>` (updated on RegisterWorker / HandleWorkerMsg) and `health: Health` (refreshed every Cmd::Tick before the view broadcast). Frontend `AvatarVM` mirrors the shape; Pixi alpha drops for non-`online` states.

**Tech Stack:** Rust (sqlx, tokio, ts-rs, serde), TypeScript (React + Pixi), vitest, Playwright.

**Spec:** `docs/superpowers/specs/2026-05-12-daemon-health-buckets-design.md`

---

## File structure

- `crates/world/src/health.rs` *(new)* — `enum Health` (Serialize + ts-rs export) + `fn derive(now_ts, last_seen, connected, is_operator) -> Health` + threshold constants + 8 inline unit tests.
- `crates/world/src/lib.rs` *(modify)* — `pub mod health;` so the new module is reachable.
- `crates/world/src/state.rs` *(modify)* — `AvatarView` gains `last_seen_at: Option<i64>` and `health: Health`. ts-rs auto-exports.
- `crates/world/src/loop_.rs` *(modify)* — `Cmd::RegisterWorker` + `Cmd::HandleWorkerMsg` set `last_seen_at = Some(unix_now())`; `Cmd::Tick` handler refreshes `health` for every avatar before `view_tx.send`. Small `fn unix_now() -> i64` helper.
- `crates/world/src/cmd_console.rs` *(modify)* — `OperatorPossess` AvatarView construction adds `last_seen_at: None, health: Health::Online`.
- `crates/world/src/api_startups.rs` *(modify)* — `create_startup`'s AvatarView construction adds `last_seen_at: None, health: Health::Offline`.
- `crates/world/tests/*.rs` *(modify, mechanical)* — every AvatarView fixture gets `last_seen_at: None, health: Health::Offline`. ~38 sites, walk via compiler errors.
- `crates/world/tests/health_integration.rs` *(new)* — 3 integration tests that boot `loop_::spawn` and assert health transitions.
- `packages/frontend/src/store.ts` *(modify)* — `AvatarVM` gains `last_seen_at: number | null` and `health` literal-union; `coerceAvatar` defensively reads both.
- `packages/frontend/src/town/PixiStage.tsx` *(modify)* — per-avatar `sprite.alpha` from a `ALPHA_BY_HEALTH` lookup.
- `CHANGELOG.md` + `TODOS.md` *(modify)* — M12 entry; TODOS Completed entry with `<TBD>` PR placeholder.

---

## Task 1: `health.rs` pure module + unit tests

**Files:**
- Create: `crates/world/src/health.rs`
- Modify: `crates/world/src/lib.rs`

- [ ] **Step 1: Create the module with all 8 unit tests written first (red)**

Write `crates/world/src/health.rs`:

```rust
//! P2.1 health derivation: 4-bucket worker liveness from
//! (now, last_seen, connected, is_operator). Pure module — no I/O.

use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Online,
    RecentlyLost,
    Offline,
    AboutToGc,
}

/// Worker is considered RecentlyLost for the first 5 minutes after WS
/// disconnect; transient network blips fall inside this window.
pub const RECENTLY_LOST_MAX_SECS: i64 = 5 * 60;

/// Beyond 5 minutes the worker is fully Offline; the bucket caps at 6 days
/// so the next bucket (about-to-gc) gets a clean 1-day window.
pub const OFFLINE_MAX_SECS: i64 = 6 * 24 * 60 * 60;

/// 7 days is the conventional GC threshold; the last 24 hours before that
/// surface as AboutToGc so the operator can intervene.
pub const ABOUT_TO_GC_MAX_SECS: i64 = 7 * 24 * 60 * 60;

/// Decide the avatar's Health state. `now_ts` is unix seconds; `last_seen`
/// is the last time we received any worker message (None if never). The
/// operator override exists because the operator avatar has no worker WS —
/// its presence in the avatars map already means the console is connected.
pub fn derive(now_ts: i64, last_seen: Option<i64>, connected: bool, is_operator: bool) -> Health {
    if is_operator {
        return Health::Online;
    }
    if connected {
        return Health::Online;
    }
    let Some(ts) = last_seen else {
        return Health::Offline;
    };
    let delta = now_ts - ts;
    if delta < 0 {
        // Clock skew / NTP step — be charitable.
        return Health::Online;
    }
    if delta <= RECENTLY_LOST_MAX_SECS {
        Health::RecentlyLost
    } else if delta <= OFFLINE_MAX_SECS {
        Health::Offline
    } else if delta <= ABOUT_TO_GC_MAX_SECS {
        Health::AboutToGc
    } else {
        Health::Offline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000;

    #[test]
    fn online_when_connected() {
        assert_eq!(derive(NOW, Some(NOW - 100), true, false), Health::Online);
        assert_eq!(derive(NOW, None, true, false), Health::Online);
    }

    #[test]
    fn online_when_operator() {
        assert_eq!(derive(NOW, None, false, true), Health::Online);
        assert_eq!(derive(NOW, Some(NOW - 10_000_000), false, true), Health::Online);
    }

    #[test]
    fn offline_when_disconnected_no_last_seen() {
        assert_eq!(derive(NOW, None, false, false), Health::Offline);
    }

    #[test]
    fn recently_lost_within_5min() {
        assert_eq!(derive(NOW, Some(NOW - 1), false, false), Health::RecentlyLost);
        assert_eq!(derive(NOW, Some(NOW - 299), false, false), Health::RecentlyLost);
        assert_eq!(derive(NOW, Some(NOW - 300), false, false), Health::RecentlyLost);
    }

    #[test]
    fn offline_after_5min() {
        assert_eq!(derive(NOW, Some(NOW - 301), false, false), Health::Offline);
        assert_eq!(derive(NOW, Some(NOW - OFFLINE_MAX_SECS), false, false), Health::Offline);
    }

    #[test]
    fn about_to_gc_after_6_days() {
        assert_eq!(
            derive(NOW, Some(NOW - OFFLINE_MAX_SECS - 1), false, false),
            Health::AboutToGc
        );
        assert_eq!(
            derive(NOW, Some(NOW - ABOUT_TO_GC_MAX_SECS), false, false),
            Health::AboutToGc
        );
    }

    #[test]
    fn offline_after_7_days() {
        assert_eq!(
            derive(NOW, Some(NOW - ABOUT_TO_GC_MAX_SECS - 1), false, false),
            Health::Offline
        );
    }

    #[test]
    fn clock_skew_treated_as_online() {
        assert_eq!(derive(NOW, Some(NOW + 10), false, false), Health::Online);
    }
}
```

- [ ] **Step 2: Wire the module into the crate**

Edit `crates/world/src/lib.rs`. Find the `pub mod` declarations (there will be many). Add `pub mod health;` in alphabetical order among them.

- [ ] **Step 3: Run the unit tests**

```bash
cargo test -p cliptown-world health::tests
```

Expected: 8 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/world/src/health.rs crates/world/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(world): health derivation module

Pure module with Health enum (4 states) and derive(now, last_seen,
connected, is_operator). Thresholds: 5min RecentlyLost, 6d Offline,
7d AboutToGc, then Offline again. Operator avatars and clock skew
both forced Online. 8 inline unit tests cover the truth table.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `AvatarView` carries `last_seen_at` + `health`

**Files:**
- Modify: `crates/world/src/state.rs`
- Modify: `crates/world/src/cmd_console.rs`
- Modify: `crates/world/src/api_startups.rs`
- Modify: every `crates/world/tests/*.rs` file that constructs `AvatarView` (use compiler errors as a checklist)

- [ ] **Step 1: Update the struct**

Edit `crates/world/src/state.rs`. Add the two fields and a `Health` import:

```rust
use std::collections::HashMap;
use serde::Serialize;
use ts_rs::TS;

use crate::health::Health;

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
    pub last_seen_at: Option<i64>,
    pub health: Health,
}
```

- [ ] **Step 2: Run the build — expect a wall of errors**

```bash
cargo build -p cliptown-world --tests 2>&1 | grep -E "error\[" | head -10
```

Expected: ~40 errors, all "missing fields `last_seen_at` and `health`". This is your checklist.

- [ ] **Step 3: Fix `cmd_console.rs::OperatorPossess`**

Find the `AvatarView {` block (around line 51) and add the two new fields. Operator gets `health: Health::Online` because the avatar only exists while the console is possessing:

```rust
let avatar = AvatarView {
    agent_id: OPERATOR_AVATAR_ID.to_string(),
    startup_id: startup_id.clone(),
    role: "operator".to_string(),
    backend: "operator".to_string(),
    current_pos: (20, 5),
    target_pos: None,
    room_id: "lobby".to_string(),
    status: "idle".to_string(),
    last_seen_at: None,
    health: Health::Online,
};
```

Add `use crate::health::Health;` to the file's imports.

- [ ] **Step 4: Fix `api_startups.rs::create_startup`**

Find the `AvatarView {` block (around line 286). Add:

```rust
            last_seen_at: None,
            health: Health::Offline,
```

Add `use cliptown_world::health::Health;` or `use crate::health::Health;` depending on whether the file uses `crate::` or external import. Check existing imports.

- [ ] **Step 5: Fix all test-file fixtures**

Run `cargo build -p cliptown-world --tests 2>&1 | grep -oE 'crates/world/tests/[^:]+:[0-9]+' | sort -u` to enumerate the test files + lines. For each, open the file, find every `AvatarView {` literal, and add at the end of the struct (before the closing `}`):

```rust
            last_seen_at: None,
            health: cliptown_world::health::Health::Offline,
```

(Use `cliptown_world::health::Health::Offline` rather than importing because tests already use the fully-qualified path for other cliptown_world types.)

Files to expect:
- `crates/world/tests/api_startups.rs`
- `crates/world/tests/budget_thresholds.rs`
- `crates/world/tests/console_emit.rs`
- `crates/world/tests/e2e_cafe.rs`
- ... and any others the grep surfaces.

Some test files have helper functions like `av_in()` in `e2e_cafe.rs` — fix the helper once, and any test calling it is automatically fixed.

- [ ] **Step 6: Verify the build is clean**

```bash
cargo build -p cliptown-world --tests
```

Expected: no errors.

- [ ] **Step 7: Run all tests to confirm no regression**

```bash
cargo test -p cliptown-world 2>&1 | tail -5
```

Expected: all existing tests still pass (219 total + 8 new health::tests = 227).

- [ ] **Step 8: Commit**

```bash
git add crates/world/src/state.rs crates/world/src/cmd_console.rs crates/world/src/api_startups.rs crates/world/tests/
git commit -m "$(cat <<'EOF'
feat(world): AvatarView carries last_seen_at + health

Two new fields on AvatarView. Operator avatar (cmd_console) defaults
to health: Online (only exists while possessing). Worker avatars
(api_startups InsertAvatars path) default to Offline (no WS yet). All
~40 test fixtures updated mechanically — both new fields go to None /
Offline since tests aren't asserting on health yet.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `last_seen_at` updates + per-tick health refresh in `loop_.rs`

**Files:**
- Modify: `crates/world/src/loop_.rs`

- [ ] **Step 1: Add `unix_now` helper + import Health**

At the top of `crates/world/src/loop_.rs`, find the imports block and add:

```rust
use crate::health::{self, Health};
```

After the imports, before the `Cmd` enum (around line 9), add:

```rust
fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
```

- [ ] **Step 2: Update `Cmd::RegisterWorker` handler**

Find the `Cmd::RegisterWorker { agent_id, tx: out_tx } => { ... }` arm (around line 139). Currently it just inserts into `out_bus`. Change it to also touch `last_seen_at`:

```rust
                Cmd::RegisterWorker { agent_id, tx: out_tx } => {
                    out_bus.insert(agent_id.clone(), out_tx);
                    if let Some(av) = w.avatars.get_mut(&agent_id) {
                        av.last_seen_at = Some(unix_now());
                    }
                }
```

- [ ] **Step 3: Update `Cmd::HandleWorkerMsg` handler**

Find the `Cmd::HandleWorkerMsg { agent_id, msg, reply } => { ... }` arm (around line 130). Add the `last_seen_at` update at the very top of the arm, BEFORE the dispatch call:

```rust
                Cmd::HandleWorkerMsg { agent_id, msg, reply } => {
                    if let Some(av) = w.avatars.get_mut(&agent_id) {
                        av.last_seen_at = Some(unix_now());
                    }
                    // ...existing dispatch logic stays unchanged below...
```

(Preserve all existing code below this insertion.)

- [ ] **Step 4: Refresh `health` on every tick**

Find the `Cmd::Tick => { ... }` arm (around line 99). At the END of the tick body, after `crate::proximity::compute_and_emit(&w, &out_bus);` and BEFORE `let _ = view_tx.send(w.clone());`, insert:

```rust
                    // P2.1: derive health bucket per avatar before broadcasting.
                    let now = unix_now();
                    for (agent_id, av) in w.avatars.iter_mut() {
                        let connected = out_bus.contains_key(agent_id);
                        let is_operator = av.role == "operator";
                        av.health = health::derive(now, av.last_seen_at, connected, is_operator);
                    }
```

- [ ] **Step 5: Build + run all tests**

```bash
cargo build -p cliptown-world
cargo test -p cliptown-world 2>&1 | tail -5
```

Expected: clean build; all existing tests still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/world/src/loop_.rs
git commit -m "$(cat <<'EOF'
feat(world): wire last_seen_at + per-tick health refresh

- RegisterWorker / HandleWorkerMsg set avatar.last_seen_at = unix_now().
- UnregisterWorker keeps last_seen_at unchanged (time aging handles the
  health transition naturally).
- Every Cmd::Tick recomputes avatar.health from (now, last_seen_at,
  out_bus connection state, role) before broadcasting the WorldView.
  Cost is constant-per-avatar; with <100 avatars in practice this is
  well within the M11 bench harness's ±20% tolerance.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Integration tests

**Files:**
- Create: `crates/world/tests/health_integration.rs`

- [ ] **Step 1: Write the integration test file**

Create `crates/world/tests/health_integration.rs`:

```rust
//! P2.1 integration tests — boot loop_::spawn, exercise Cmd handlers,
//! observe last_seen_at + health transitions via view_rx.

use cliptown_world::health::Health;
use cliptown_world::loop_::{self, Cmd};
use cliptown_world::state::{AvatarView, WorldView};
use cliptown_world::storage;
use tokio::sync::{broadcast, mpsc, oneshot};

async fn boot() -> (loop_::Handle, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = storage::open(dir.path().join("test.db").to_str().unwrap())
        .await
        .unwrap();
    cliptown_world::seed::seed_if_empty(&pool).await.unwrap();
    let (event_tx, _event_rx) = broadcast::channel(64);
    let handle = loop_::spawn(WorldView::default(), pool, event_tx);
    (handle, dir)
}

fn fake_avatar(agent_id: &str, role: &str) -> AvatarView {
    AvatarView {
        agent_id: agent_id.to_string(),
        startup_id: "s1".to_string(),
        role: role.to_string(),
        backend: "claude_code".to_string(),
        current_pos: (0, 0),
        target_pos: None,
        room_id: "lobby".to_string(),
        status: "idle".to_string(),
        last_seen_at: None,
        health: Health::Offline,
    }
}

async fn snapshot(handle: &loop_::Handle) -> WorldView {
    // Trigger a tick so health refresh fires, then wait for the broadcast.
    let mut view_rx = handle.view_rx.clone();
    handle.tx.send(Cmd::Tick).await.unwrap();
    view_rx.changed().await.unwrap();
    view_rx.borrow().clone()
}

#[tokio::test]
async fn register_sets_last_seen_and_marks_online() {
    let (handle, _dir) = boot().await;
    handle
        .tx
        .send(Cmd::InsertAvatars {
            avatars: vec![fake_avatar("a1", "engineer")],
            claim_suite: None,
        })
        .await
        .unwrap();
    let (out_tx, _out_rx) = mpsc::channel::<serde_json::Value>(8);
    handle
        .tx
        .send(Cmd::RegisterWorker {
            agent_id: "a1".to_string(),
            tx: out_tx,
        })
        .await
        .unwrap();

    let view = snapshot(&handle).await;
    let av = view.avatars.get("a1").expect("avatar a1 present");
    assert!(av.last_seen_at.is_some(), "register should set last_seen_at");
    assert_eq!(av.health, Health::Online);
}

#[tokio::test]
async fn handle_msg_refreshes_last_seen() {
    let (handle, _dir) = boot().await;
    handle
        .tx
        .send(Cmd::InsertAvatars {
            avatars: vec![fake_avatar("a1", "engineer")],
            claim_suite: None,
        })
        .await
        .unwrap();
    let (out_tx, _out_rx) = mpsc::channel::<serde_json::Value>(8);
    handle
        .tx
        .send(Cmd::RegisterWorker {
            agent_id: "a1".to_string(),
            tx: out_tx,
        })
        .await
        .unwrap();
    let v0 = snapshot(&handle).await;
    let ts0 = v0.avatars["a1"].last_seen_at.unwrap();

    // Wait long enough for unix_now() to advance by at least 1 second.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let (reply_tx, _reply_rx) = oneshot::channel();
    handle
        .tx
        .send(Cmd::HandleWorkerMsg {
            agent_id: "a1".to_string(),
            msg: serde_json::json!({"type":"noop"}),
            reply: reply_tx,
        })
        .await
        .unwrap();

    let v1 = snapshot(&handle).await;
    let ts1 = v1.avatars["a1"].last_seen_at.unwrap();
    assert!(
        ts1 > ts0,
        "HandleWorkerMsg should refresh last_seen_at (was {}, now {})",
        ts0,
        ts1
    );
    assert_eq!(v1.avatars["a1"].health, Health::Online);
}

#[tokio::test]
async fn unregister_preserves_last_seen_marks_recently_lost() {
    let (handle, _dir) = boot().await;
    handle
        .tx
        .send(Cmd::InsertAvatars {
            avatars: vec![fake_avatar("a1", "engineer")],
            claim_suite: None,
        })
        .await
        .unwrap();
    let (out_tx, _out_rx) = mpsc::channel::<serde_json::Value>(8);
    handle
        .tx
        .send(Cmd::RegisterWorker {
            agent_id: "a1".to_string(),
            tx: out_tx,
        })
        .await
        .unwrap();
    let v0 = snapshot(&handle).await;
    let ts0 = v0.avatars["a1"].last_seen_at.unwrap();

    handle
        .tx
        .send(Cmd::UnregisterWorker {
            agent_id: "a1".to_string(),
        })
        .await
        .unwrap();

    let v1 = snapshot(&handle).await;
    let av = v1.avatars.get("a1").expect("avatar still present");
    // last_seen_at must be preserved (>= ts0; time may have advanced via
    // the post-unregister tick refresh, but no Cmd has touched the field).
    assert_eq!(av.last_seen_at, Some(ts0), "last_seen_at preserved across unregister");
    // Disconnected just now → still inside the recently_lost window.
    assert_eq!(av.health, Health::RecentlyLost);
}
```

- [ ] **Step 2: Run the new integration tests**

```bash
cargo test -p cliptown-world --test health_integration 2>&1 | tail -10
```

Expected: 3 tests pass.

- [ ] **Step 3: Run the full suite to confirm no regression**

```bash
cargo test -p cliptown-world 2>&1 | tail -5
```

Expected: 230 tests pass (219 original + 8 health unit + 3 integration).

- [ ] **Step 4: Commit**

```bash
git add crates/world/tests/health_integration.rs
git commit -m "$(cat <<'EOF'
test(world): integration tests for health bucket transitions

3 tests boot loop_::spawn end-to-end and assert:
- RegisterWorker sets last_seen_at; health flips to Online.
- HandleWorkerMsg refreshes last_seen_at across a 1.1s sleep.
- UnregisterWorker preserves last_seen_at; health flips to
  RecentlyLost (still inside the 5-minute window).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Frontend AvatarVM + Pixi alpha

**Files:**
- Modify: `packages/frontend/src/store.ts`
- Modify: `packages/frontend/src/town/PixiStage.tsx`

- [ ] **Step 1: Extend `AvatarVM` in `store.ts`**

Find the `export interface AvatarVM { ... }` block (around line 25). Add two fields at the end:

```ts
export interface AvatarVM {
  agent_id: string;
  startup_id: string;
  role: string;
  backend: string;
  current_pos: [number, number];
  target_pos: [number, number] | null;
  room_id: string;
  status: string;
  last_seen_at: number | null;
  health: "online" | "recently_lost" | "offline" | "about_to_gc";
}
```

- [ ] **Step 2: Update `coerceAvatar` to read the new fields defensively**

`coerceAvatar` at line 181 currently has this shape:

```ts
function coerceAvatar(a: Record<string, unknown>, agent_id: string): AvatarVM {
  const cp = a.current_pos;
  const tp = a.target_pos;
  return {
    agent_id: asString(a.agent_id, agent_id),
    startup_id: asString(a.startup_id),
    role: asString(a.role),
    backend: asString(a.backend),
    current_pos: ...,
    target_pos: ...,
    room_id: asString(a.room_id),
    status: asString(a.status),
  };
}
```

Add `last_seen_at` and `health` to the returned object literal. The final function should be:

```ts
function coerceAvatar(a: Record<string, unknown>, agent_id: string): AvatarVM {
  const cp = a.current_pos;
  const tp = a.target_pos;
  const healthRaw = typeof a.health === "string" ? a.health : "offline";
  const VALID: ReadonlyArray<AvatarVM["health"]> = [
    "online", "recently_lost", "offline", "about_to_gc",
  ];
  const health: AvatarVM["health"] = (VALID as ReadonlyArray<string>).includes(healthRaw)
    ? (healthRaw as AvatarVM["health"])
    : "offline";
  return {
    agent_id: asString(a.agent_id, agent_id),
    startup_id: asString(a.startup_id),
    role: asString(a.role),
    backend: asString(a.backend),
    current_pos: Array.isArray(cp) && cp.length >= 2 && typeof cp[0] === "number" && typeof cp[1] === "number"
      ? [cp[0], cp[1]]
      : [0, 0],
    target_pos: Array.isArray(tp) && tp.length >= 2 && typeof tp[0] === "number" && typeof tp[1] === "number"
      ? [tp[0], tp[1]]
      : null,
    room_id: asString(a.room_id),
    status: asString(a.status),
    last_seen_at: typeof a.last_seen_at === "number" ? a.last_seen_at : null,
    health,
  };
}
```

- [ ] **Step 3: Apply Pixi alpha in `PixiStage.tsx`**

The per-avatar update loop lives around lines 171-194 (the `for (const raw of ours)` block). Each iter normalizes the snapshot avatar, finds/creates a sprite. Operator alpha is driven by the possess transition at line 131 (`opSprite.container.alpha = operatorAlpha(possessRef.current, now)`) — it must NOT be overwritten here. For non-operator avatars we apply the health bucket.

a. Add a module-level const at the top of `PixiStage.tsx` (after the imports, before the component function). If `AvatarVM` isn't yet imported, add `import type { AvatarVM } from "../store.js";`. Then:

```ts
const ALPHA_BY_HEALTH: Record<AvatarVM["health"], number> = {
  online: 1.0,
  recently_lost: 0.7,
  offline: 0.4,
  about_to_gc: 0.3,
};
```

b. In the avatar update loop (the `for (const raw of ours) { ... }` block around lines 171-194), at the END of the loop body (just before the closing `}`), add:

```ts
      // P2.1: health-driven alpha for non-operator avatars. Operator alpha
      // is driven separately by the possess transition (see operatorAlpha
      // call above) and must not be clobbered here.
      if (a.agent_id !== OPERATOR_AVATAR_ID) {
        const sprite = spritesRef.current.get(a.agent_id);
        if (sprite) {
          sprite.container.alpha = ALPHA_BY_HEALTH[a.health];
        }
      }
```

This works whether the iter took the `existing` branch (`updateAvatarTargets` path) or the new-sprite branch — both leave a sprite in the `spritesRef` map, so the lookup succeeds in both cases.

- [ ] **Step 4: Run frontend tests**

```bash
pnpm -F @cliptown/frontend e2e 2>&1 | tail -10
```

Expected: 14 Playwright tests pass. The store's defensive defaults in `coerceAvatar` mean even snapshots from old fixtures (without `health`) work — `health` defaults to `"offline"`, alpha defaults to 0.4. Existing tests don't assert on alpha so no breakage.

- [ ] **Step 5: Commit**

```bash
git add packages/frontend/src/store.ts packages/frontend/src/town/PixiStage.tsx
git commit -m "$(cat <<'EOF'
feat(frontend): AvatarVM carries health; Pixi alpha reflects it

AvatarVM gains last_seen_at + health fields paralleling the Rust shape.
coerceAvatar reads both defensively (unknown shapes degrade to null /
"offline"). PixiStage applies sprite.alpha from a per-bucket lookup:
online 1.0, recently_lost 0.7, offline 0.4, about_to_gc 0.3.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: CHANGELOG + TODOS + verification sweep

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `TODOS.md`

- [ ] **Step 1: Add the M12 section at the top of CHANGELOG.md**

Find the line `## M11 — real bench harness (2026-05-12)` near the top. Insert this NEW section ABOVE it:

```markdown
## M12 — P2.1 daemon health buckets (2026-05-12)

Replaces cliptown's binary worker-liveness signal (WS connected vs
closed) with a 4-state Health enum so the operator console doesn't
confuse a 5-minute network blip with a hard crash.

- **`crates/world/src/health.rs`** (new pure module) — `Health` enum
  + `derive(now_ts, last_seen, connected, is_operator) -> Health`.
  Thresholds: `RecentlyLost` ≤ 5 min, `Offline` ≤ 6 d,
  `AboutToGc` ≤ 7 d (last 24 h before GC), beyond 7 d back to
  `Offline`. Operator avatars and clock skew both forced Online.
- **`AvatarView`** carries `last_seen_at: Option<i64>` (updated on
  `RegisterWorker` / `HandleWorkerMsg`, preserved through
  `UnregisterWorker`) and `health: Health` (refreshed every
  `Cmd::Tick` before the view broadcast).
- **Frontend `AvatarVM`** mirrors the shape; `PixiStage.tsx` sets
  `sprite.alpha` from `ALPHA_BY_HEALTH` (`online: 1.0`,
  `recently_lost: 0.7`, `offline: 0.4`, `about_to_gc: 0.3`).
- **Tests:** 8 inline unit tests for `health::derive` + 3 integration
  tests booting `loop_::spawn` (register sets last_seen + Online;
  HandleWorkerMsg refreshes; Unregister preserves + RecentlyLost).

```

- [ ] **Step 2: Add the TODOS entry**

Open `TODOS.md`. Under `## Completed`, ABOVE the existing `### M11 real bench harness` entry, insert:

```markdown
### M12 P2.1 daemon health buckets — 2026-05-12
**Source:** Phase 2 backlog first item (from `docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md` § P2.1). PR `<TBD — fill in at PR creation>`.

Was: cliptown's operator console treated worker liveness as binary (WS connected vs closed). A 5-minute network blip looked identical to a hard crash, generating noise.

Fixed: `AvatarView` now carries `last_seen_at: Option<i64>` (updated on RegisterWorker / HandleWorkerMsg) and `health: Health` (derived per tick from connection state + age of last_seen). 4 states — `online` / `recently_lost` / `offline` / `about_to_gc` — replace the binary signal. New `crates/world/src/health.rs` pure module owns derivation + thresholds. Frontend `AvatarVM` mirrors the shape; Pixi alpha dims non-online avatars. 11 new tests (8 unit + 3 integration).

```

The PR number gets filled in after `gh pr create`.

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md TODOS.md
git commit -m "$(cat <<'EOF'
docs: M12 P2.1 daemon health buckets changelog + TODOS

Adds the M12 section atop CHANGELOG (Phase 2 begins). TODOS Completed
gets the matching entry with TBD PR placeholder.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 4: Full verification sweep**

Run from the repo root:

```bash
cargo test -p cliptown-world 2>&1 | grep "test result:" | awk '{sum += $4} END {print "rust:", sum}'
pnpm -F @cliptown/adapter-core test 2>&1 | tail -2 | head -1
pnpm -F @cliptown/adapter-claude-code test 2>&1 | tail -2 | head -1
pnpm -F @cliptown/adapter-codex test 2>&1 | tail -2 | head -1
pnpm -F @cliptown/adapter-opencode test 2>&1 | tail -2 | head -1
pnpm -F @cliptown/worker test 2>&1 | tail -2 | head -1
pnpm -F @cliptown/frontend e2e 2>&1 | tail -2 | head -1
node bench/check.mjs 2>&1 | head -20
```

Expected:
- rust: 230 (was 219, +8 unit +3 integration)
- adapter-core: 3
- adapter-claude-code: 8
- adapter-codex: 12
- adapter-opencode: 12
- worker: 65
- frontend e2e: 14 passed
- `node bench/check.mjs` — exit 0; `tick_latency_real_loop` delta within ±20% (the per-tick health derive adds µs).

If the bench gate trips, the implementation has accidentally introduced an expensive operation in the tick path — investigate. Likely culprit: an extra clone, a SQL call, or a HashMap rebuild instead of in-place mutation.

No commit in this step — pure verification.

---

## Definition of done

- `crates/world/src/health.rs` exists with the Health enum, derive fn, thresholds, and 8 inline unit tests — all green.
- `AvatarView` has `last_seen_at` + `health` fields; every construction site (production + ~38 test fixtures) compiles.
- `loop_.rs` sets `last_seen_at` on `RegisterWorker` / `HandleWorkerMsg`, preserves it on `UnregisterWorker`, refreshes `health` every `Cmd::Tick`.
- `crates/world/tests/health_integration.rs` — 3 tests green.
- `cargo test -p cliptown-world` — 230 tests total.
- `cargo bench -p cliptown-world` — `tick_latency_real_loop` within ±20% of baseline.
- Frontend `AvatarVM` carries `health`; `coerceAvatar` defensively reads it; PixiStage applies alpha.
- `pnpm -F @cliptown/frontend e2e` — 14 green.
- CHANGELOG carries the M12 entry; TODOS Completed has the matching entry (PR number filled at PR-create time).
