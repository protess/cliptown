# Real bench harness — design

**Date:** 2026-05-12
**Status:** draft — pending implementation
**Driver:** Phase 1 known-limitation cleanup. The two criterion benches in
`crates/world/benches/world_bench.rs` are placeholders:
`tick_latency_per_loop_iter` runs `sum 0..1000` inside a tokio runtime;
`mpsc_throughput_1k_msgs` measures an in-process mpsc channel with no
relation to the world's actual hot path. Both are documented as Phase 0
shims awaiting the real harness.

This spec swaps the bench bodies for `loop_::spawn`-driven measurements,
re-baselines the JSON gate, and retires the Phase 0 "criterion benches
are placeholders" line from the changelog.

## Goals

- `tick_latency` measures one real `Cmd::Tick` round-trip through the
  spawned world loop, including `move_sys::step_all`,
  `scheduler::tick`, `proximity::compute_and_emit`, and the
  `view_tx.send` broadcast.
- `console_dispatch_throughput` measures sustained
  `Cmd::HandleConsoleMsg` round-trip throughput — the same lane every
  frontend / operator command travels.
- `bench/baselines.json` reflects the new bench shapes with fresh
  numbers captured on the dev box.
- `bench/check.mjs` knows the new extract recipe.
- "Two criterion benches are still placeholders" retires from
  CHANGELOG's Phase-1 known-limitations list.

## Non-goals (explicit)

- New bench dimensions (per-startup proximity throughput, scheduler
  fairness, sandbox path resolution latency, etc.).
- CI baseline calibration vs ubuntu-latest — the gate stays
  `continue-on-error: true` until more CI samples land.
- Hard gate flip (`continue-on-error: false`). Separate follow-up.
- Frontend FCP bench — already closed under PR #35; no changes here.

## Architecture

`crates/world/benches/world_bench.rs` keeps the same shape (one
`criterion_group` with two `bench_function` targets), but each function
body is rewritten around a real `loop_::spawn` handle.

### `tick_latency_real_loop`

Per-bench setup (outside `b.iter`):

1. tempdir + `storage::open` + `seed::seed_if_empty` (mirrors
   `tests/common/mod.rs::TestCtx::new`).
2. `broadcast::channel(64)` for event bus.
3. `loop_::spawn(WorldView::default(), pool, event_tx)` → `handle`.
4. Hold the tempdir alive for the lifetime of the bench function so
   the SQLite file doesn't vanish mid-run.

Per iter:

```rust
b.iter(|| {
    rt.block_on(async {
        let _ = handle.tx.send(Cmd::Tick).await;
        // view_tx fires once per Cmd::Tick handler, so changed() returns
        // exactly when the tick has fully processed (move_sys + scheduler +
        // proximity + view send all complete).
        let mut view_rx = handle.view_rx.clone();
        let _ = view_rx.changed().await;
    });
});
```

We clone the watch receiver per iter so its "has changed" flag starts
fresh — without this, the first `changed().await` returns immediately
because the watch channel pre-seeds with the initial view.

### `console_dispatch_throughput_100_msgs`

Same setup. Per iter:

```rust
b.iter(|| {
    rt.block_on(async {
        let mut replies = Vec::with_capacity(100);
        for _ in 0..100 {
            let (tx, rx) = oneshot::channel();
            // Use a shape the dispatcher will reject quickly so we
            // measure dispatch+reply round-trip, not side-effect cost.
            let msg = serde_json::json!({"type":"bench_noop"});
            handle.tx.send(Cmd::HandleConsoleMsg { msg, reply: tx }).await.ok();
            replies.push(rx);
        }
        for rx in replies { let _ = rx.await; }
    });
});
```

The `"bench_noop"` shape is unknown to `cmd_console::dispatch`. The
dispatcher first tries `serde_json::from_value::<ConsoleInbound>(msg)`
and returns `{"type":"error","reason":"parse",...}` on failure (see
`crates/world/src/cmd_console.rs:28-31`). The reply fires before any
DB write or broadcast, so the bench measures only the mpsc → parse →
oneshot round-trip — the same hot path every real console command
travels for its initial dispatch hop.

## Bench naming + check.mjs

Bench function names in `world_bench.rs`:

- `tick_latency_real_loop`
- `console_dispatch_throughput_100_msgs`

`bench/baselines.json` metric keys:

- `world.tick_latency_us` (key unchanged; only `criterion_bench` value updates)
- `world.console_dispatch_throughput_msgs_per_sec` (was
  `world.mpsc_throughput_msgs_per_sec`)

`bench/check.mjs`:

- Add `100_div_median_us` extract recipe:
  ```js
  case "100_div_median_us":
    return 100 / (medianNs / 1_000 / 1_000_000);
  ```
- Drop the now-unused `1000_div_median_us` case.

## Re-baselining procedure

1. Land the bench-body rewrite + check.mjs change first, with
   placeholder baseline values (e.g. `"baseline": 0`).
2. Run `cargo bench -p cliptown-world` on the dev box.
3. Read criterion's `target/criterion/<bench>/new/estimates.json` for
   each bench, grab `median.point_estimate` (nanoseconds).
4. Apply the extract recipe in `bench/check.mjs` to convert to baseline
   units. Update `bench/baselines.json` with the rounded value.
5. Re-run `node bench/check.mjs` to confirm the gate passes with `delta_pct ≈ 0`.

This is a one-shot operator step at PR-author time; CI just consumes
the file.

## CHANGELOG

The Phase-1 "Known limitations" line that reads:

> Two criterion benches (`cargo bench -p cliptown-world`) are still
> placeholders: tick latency = sum 0..1000, throughput = in-process
> 1k-msg mpsc. Phase 1 swaps in real `loop_::spawn`-driven harnesses.

retires from the list. The `bench.yml` `continue-on-error: true` line
stays — that's a separate item.

A new entry goes under a new `## M11 — real bench harness (2026-05-12)`
section at the top (sibling to the M11 hook bridge entry that shipped
2026-05-12).

## Testing

The benches themselves don't run under `cargo test`. The validation
plan:

- `cargo build --benches -p cliptown-world` compiles cleanly.
- `cargo bench -p cliptown-world` produces medians for both benches.
- `node bench/check.mjs` exits 0 with `delta_pct ≈ 0` after fresh
  baselines land.
- Existing `cargo test -p cliptown-world` (219 tests) stays green —
  this PR touches only `benches/`, `bench/baselines.json`, and
  `bench/check.mjs`; no library code.

## Risk

- Bench setup cost (sqlx pool open + seed) is paid once per bench
  function, but criterion may re-enter setup between sample groups.
  Acceptable: setup is ~ms and criterion's measurement_time of 3s
  gives many iterations per sample.
- `view_rx.changed()` in the latency bench relies on the loop sending
  `view_tx.send(w.clone())` after every Cmd::Tick. Verified in
  `loop_.rs:123`.
- `Cmd::HandleConsoleMsg` with unknown shape returns a reply via the
  dispatcher's parse-error path
  (`crates/world/src/cmd_console.rs:28-31`); verified at design time.

## Definition of done

- `crates/world/benches/world_bench.rs` body rewritten; old `sum 0..1000`
  / `tokio::mpsc 1k msgs` patterns gone.
- `bench/baselines.json` has both metrics with fresh numbers and the
  `console_dispatch_throughput_msgs_per_sec` rename.
- `bench/check.mjs` has the new extract recipe and drops the old one.
- `cargo bench -p cliptown-world` runs without errors.
- `node bench/check.mjs` exits 0 against the freshly-captured
  baselines.
- `cargo test -p cliptown-world` still 219 green.
- CHANGELOG retires the placeholder note and adds the M11 bench entry.
- TODOS.md Completed gets an entry for the M11 bench harness work.
