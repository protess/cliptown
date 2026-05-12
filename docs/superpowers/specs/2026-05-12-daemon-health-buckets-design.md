# P2.1 daemon health buckets — design

**Date:** 2026-05-12
**Status:** draft — pending implementation
**Driver:** Phase 2 backlog first item from `docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md` § Phase 2 → P2.1. Today cliptown's operator console treats worker liveness as binary (WS connected vs WS closed). A 5-minute network blip looks identical to a hard crash. 4-state buckets collapse transient noise and surface "this worker is actually dead" only when warranted.

## Goals

- Each `AvatarView` carries a `health: Health` field in one of four states: `online` / `recently_lost` / `offline` / `about_to_gc`.
- World derives `health` per tick from (WS-connection state, `last_seen_at`, role). Frontend reads — no thresholds duplicated in the frontend.
- Pixi canvas dims non-`online` avatars (alpha 0.7 / 0.4 / 0.3) so operators see liveness at a glance.
- No protocol change. No worker code change. Implicit heartbeat = any inbound worker message.

## Non-goals (explicit)

- Explicit heartbeat protocol (worker sending `{"type":"heartbeat"}` every N seconds). Out of scope; current message-driven inference suffices.
- SQL persistence of `last_seen_at`. In-memory only — process restart resets to `None`. Tracking liveness across restarts is a different problem.
- Actual GC execution. The `about_to_gc` state is informational; real GC remains a separate (future) concern.
- Operator-console badge / list view changes. One visual surface (Pixi alpha) is enough for v1.

## Architecture

Four states, derived from three inputs.

### `Health` enum (`crates/world/src/health.rs`)

```rust
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Online,
    RecentlyLost,
    Offline,
    AboutToGc,
}
```

Pure module with one public function:

```rust
pub fn derive(now_ts: i64, last_seen: Option<i64>, connected: bool, is_operator: bool) -> Health;
```

Thresholds as `const` (also pub for tests):

```rust
pub const RECENTLY_LOST_MAX_SECS: i64 = 5 * 60;         // 300
pub const OFFLINE_MAX_SECS:       i64 = 6 * 24 * 60 * 60; // 518_400 (6 days)
pub const ABOUT_TO_GC_MAX_SECS:   i64 = 7 * 24 * 60 * 60; // 604_800 (7 days)
```

### Derivation rules

```
if is_operator → Online
if connected   → Online
match last_seen:
  None → Offline
  Some(ts):
    delta = now_ts - ts
    delta < 0                         → Online        (clock skew safety)
    delta <= RECENTLY_LOST_MAX_SECS   → RecentlyLost
    delta <= OFFLINE_MAX_SECS         → Offline
    delta <= ABOUT_TO_GC_MAX_SECS     → AboutToGc
    else                              → Offline       (>7d; eligible for GC)
```

Note the `>7d → Offline` choice: once an avatar exits the about-to-gc window it's effectively gone for monitoring purposes. The state doesn't loop back; it just stops escalating.

## `AvatarView` changes (`crates/world/src/state.rs`)

Add two fields:

```rust
pub struct AvatarView {
    // ...existing fields...
    pub last_seen_at: Option<i64>,
    pub health: Health,
}
```

ts-rs auto-exports the new shape to `packages/protocol/dist/AvatarView.ts`. The `Health` enum exports to its own `Health.ts`.

### Initialization defaults

Existing AvatarView construction sites:
- `crates/world/src/cmd_console.rs::OperatorPossess` — creates the operator avatar. Default: `last_seen_at: None`, `health: Health::Online` (operator is by definition online when console is connected; `is_operator` later overrides anyway).
- `crates/world/src/api_startups.rs::create_startup` → `Cmd::InsertAvatars` path — fresh worker avatars. Default: `last_seen_at: None`, `health: Health::Offline`.
- Tests that construct `AvatarView` directly — update via compiler errors.

The compiler will fail every site that doesn't initialize both new fields. Use that as the migration checklist.

## `last_seen_at` update rules (`crates/world/src/loop_.rs`)

Three Cmd handlers touch the field:

- `Cmd::RegisterWorker { agent_id, .. }` — if `w.avatars[agent_id]` exists, set `last_seen_at = Some(now)`. Out_bus gets the new sender (existing behavior).
- `Cmd::HandleWorkerMsg { agent_id, .. }` — same `last_seen_at = Some(now)` update before dispatching to `mcp_dispatch`.
- `Cmd::UnregisterWorker { agent_id }` — remove from out_bus only. `last_seen_at` stays. Health bucket transitions naturally as time passes.

The "now" timestamp comes from `std::time::SystemTime::now()` via a small helper (`fn unix_now() -> i64`) so tests can mock if needed — but the bench harness already pins setup; tests will use real time.

## Per-tick health refresh

In the `Cmd::Tick` handler in `loop_::spawn`, AFTER `move_sys::step_all` + `scheduler::tick` + `proximity::compute_and_emit` and BEFORE `view_tx.send(w.clone())`, iterate avatars and assign `health = derive(now, last_seen_at, out_bus.contains_key(agent_id), role == "operator")`.

Avatar count is small (<100 in practice), and `derive` is branchless math — total cost is µs per tick. The M11 bench harness's `tick_latency_real_loop` will catch any regression.

## Frontend changes

### `packages/frontend/src/store.ts::AvatarVM`

Add two fields paralleling the Rust shape:

```ts
export interface AvatarVM {
  // ...existing fields...
  last_seen_at: number | null;
  health: "online" | "recently_lost" | "offline" | "about_to_gc";
}
```

`coerceAvatar` reads them defensively:

```ts
const last_seen_at = typeof raw.last_seen_at === "number" ? raw.last_seen_at : null;
const healthRaw = typeof raw.health === "string" ? raw.health : "offline";
const health = ["online", "recently_lost", "offline", "about_to_gc"].includes(healthRaw)
  ? (healthRaw as AvatarVM["health"])
  : "offline";
```

This keeps the frontend tolerant of older snapshots / replays.

### Pixi rendering (`packages/frontend/src/town/PixiStage.tsx`)

Where avatar sprites are placed in the avatar layer, set `sprite.alpha` based on `avatar.health`:

```ts
const ALPHA_BY_HEALTH: Record<AvatarVM["health"], number> = {
  online: 1.0,
  recently_lost: 0.7,
  offline: 0.4,
  about_to_gc: 0.3,
};
sprite.alpha = ALPHA_BY_HEALTH[avatar.health];
```

The exact insertion point depends on existing render loop structure — locate the per-avatar sprite update step and add this assignment alongside position/visibility updates.

## Tests

### Unit (`crates/world/src/health.rs` inline `#[cfg(test)] mod tests`)

Eight cases covering the truth table:

1. `online_when_connected` — `derive(now, Some(now - 100), connected=true, false) == Online`.
2. `online_when_operator` — `derive(now, None, connected=false, is_operator=true) == Online`.
3. `offline_when_disconnected_no_last_seen` — `derive(now, None, false, false) == Offline`.
4. `recently_lost_within_5min` — `derive(now, Some(now-299), false, false) == RecentlyLost`; same with `now-300`.
5. `offline_after_5min` — `derive(now, Some(now-301), false, false) == Offline`; same with `now-OFFLINE_MAX_SECS`.
6. `about_to_gc_after_6_days` — `derive(now, Some(now-OFFLINE_MAX_SECS-1), false, false) == AboutToGc`; same with `now-ABOUT_TO_GC_MAX_SECS`.
7. `offline_after_7_days` — `derive(now, Some(now-ABOUT_TO_GC_MAX_SECS-1), false, false) == Offline`.
8. `clock_skew_treated_as_online` — `derive(now, Some(now+10), false, false) == Online`.

### Integration (`crates/world/tests/health_integration.rs`)

Three tests using `tests/common/mod.rs::TestCtx::new` + `loop_::spawn`:

1. `register_sets_last_seen` — InsertAvatars → RegisterWorker → next tick → view's avatar has `last_seen_at = Some(_)`, `health = Online`.
2. `handle_msg_refreshes_last_seen` — Register → manually rewind `last_seen_at` (or wait briefly via tokio::time::sleep) → HandleWorkerMsg → next tick → `last_seen_at` is fresher than before.
3. `unregister_preserves_last_seen` — Register → Unregister → next tick → out_bus empty, `last_seen_at` still Some(_), `health = RecentlyLost` (just-disconnected).

### Frontend

Existing 14 Playwright tests run unchanged. The new `health` field has a defensive default in `coerceAvatar` so older snapshots don't crash. No new e2e tests added — UI change is alpha-only, visual regression is sufficient.

## Migration / risk

### Compatibility

- `AvatarView` change is type-additive; all existing serialization works because new fields appear in JSON unconditionally. Frontend reads them defensively.
- ts-rs export regenerates `packages/protocol/dist/*.ts` on `cargo build --features _ts_export`. Frontend doesn't actually import these (it mirrors the shape in `store.ts`), so the regeneration is informational.

### Performance

- Per-tick health derivation is O(avatars) with constant per-avatar cost. <100 avatars → <100 µs/tick. Tick latency bench should not regress beyond the ±20% tolerance.
- View clone per tick already copies the avatar map; the new `health` field is one more byte per avatar.

### Edge cases

- Operator avatar (`role == "operator"`) is forced to `Online` regardless of `last_seen_at`. Its existence in `state.avatars` only happens between `OperatorPossess` and `OperatorUnpossess` console commands — so when present, it's by definition active.
- Avatars created via `InsertAvatars` before any worker connects have `last_seen_at = None` and start as `Offline`. As soon as a worker connects via WS, the next tick flips them to `Online`. Brief flicker (≤1 tick = 1 second) is acceptable.

## Definition of done

- `cargo test -p cliptown-world` — all existing 219 tests green + 11 new (8 unit + 3 integration).
- `cargo bench -p cliptown-world` — `tick_latency_real_loop` median within ±20% of baseline (22.966 µs).
- `pnpm -F @cliptown/frontend e2e` — 14 green.
- `pnpm -F @cliptown/worker test` — 65 green (no worker code touched).
- Manual smoke: spawn a worker via `scripts/smoke-real-llm.sh BACKEND=claude_code`, observe the operator avatar at `online`; SIGKILL the worker process, wait 5+ minutes, confirm the avatar's Pixi sprite alpha drops (visual confirmation).
- CHANGELOG entry under M12 (Phase 2 begins).
- TODOS Completed entry.
