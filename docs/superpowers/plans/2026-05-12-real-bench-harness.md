# Real bench harness implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the two placeholder benches in `crates/world/benches/world_bench.rs` with measurements that drive a real `loop_::spawn` world handle.

**Architecture:** `tick_latency_real_loop` times one `Cmd::Tick` round-trip through the spawned loop (move_sys + scheduler + proximity + view send). `console_dispatch_throughput_100_msgs` fires 100 `Cmd::HandleConsoleMsg` with oneshot replies through the same dispatcher lane real console commands use. `bench/check.mjs` learns a new `100_div_median_us` extract recipe; `bench/baselines.json` gets fresh numbers; CHANGELOG retires the Phase-0 placeholder note.

**Tech Stack:** Rust, criterion 0.5, tokio (broadcast/oneshot/mpsc), sqlx + SQLite (via existing `cliptown_world::storage`), Node 20+ for `bench/check.mjs`.

**Spec:** `docs/superpowers/specs/2026-05-12-real-bench-harness-design.md`

---

## File structure

- `crates/world/benches/world_bench.rs` *(rewrite both bench bodies)* — imports `cliptown_world::{loop_, state, storage, seed, protocol::ConsoleOutbound}`, builds the fixture once per bench function, drives criterion's `b.iter` with real round-trips.
- `bench/check.mjs` *(modify)* — drop the `1000_div_median_us` extract recipe arm, add a `100_div_median_us` arm.
- `bench/baselines.json` *(modify)* — rename the `world.mpsc_throughput_msgs_per_sec` key to `world.console_dispatch_throughput_msgs_per_sec`; point both metrics at the new bench names; values refreshed from a real `cargo bench` run in Task 3.
- `CHANGELOG.md` *(modify)* — add a `## M11 — real bench harness (2026-05-12)` section at the top; retire the Phase-1 known-limitation bullet about placeholder benches.
- `TODOS.md` *(modify)* — add a Completed entry for M11 real bench harness.

---

## Task 1: Rewrite bench bodies around real `loop_::spawn`

**Files:**
- Modify: `crates/world/benches/world_bench.rs`

- [ ] **Step 1: Replace the entire file contents**

Use the Write tool to overwrite `crates/world/benches/world_bench.rs` with:

```rust
//! M11 real bench harness. Both benches drive a real `loop_::spawn` world
//! handle. `tick_latency_real_loop` measures one Cmd::Tick round-trip
//! (move_sys::step_all + scheduler::tick + proximity::compute_and_emit +
//! view_tx.send). `console_dispatch_throughput_100_msgs` fires 100
//! Cmd::HandleConsoleMsg with oneshot replies — the same dispatcher lane
//! frontend / operator commands travel.

use cliptown_world::{loop_, protocol::ConsoleOutbound, seed, state::WorldView, storage};
use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Duration;
use tempfile::TempDir;
use tokio::runtime::Runtime;
use tokio::sync::{broadcast, oneshot};

/// Build a real cliptown world: tmpdir SQLite, seeded schema, broadcast
/// channel, and a spawned `loop_::spawn` handle. Returns the handle plus
/// the tempdir guard (must be held alive for the lifetime of the bench).
async fn make_world() -> (loop_::Handle, TempDir, broadcast::Receiver<ConsoleOutbound>) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("bench.db");
    let pool = storage::open(db_path.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    let (event_tx, event_rx) = broadcast::channel(64);
    let handle = loop_::spawn(WorldView::default(), pool, event_tx);
    (handle, dir, event_rx)
}

fn bench_tick_latency(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    // Setup once per bench function; held alive for all iters.
    let (handle, _dir, _event_rx) = rt.block_on(make_world());

    c.bench_function("tick_latency_real_loop", |b| {
        b.iter(|| {
            rt.block_on(async {
                // Cloning the watch::Receiver per-iter resets its "has-changed"
                // flag so the first .changed() call after sending Cmd::Tick
                // actually waits for the next view broadcast (the watch channel
                // pre-seeds with the initial view).
                let mut view_rx = handle.view_rx.clone();
                handle.tx.send(loop_::Cmd::Tick).await.unwrap();
                view_rx.changed().await.unwrap();
            });
        });
    });
}

fn bench_console_dispatch_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (handle, _dir, _event_rx) = rt.block_on(make_world());

    c.bench_function("console_dispatch_throughput_100_msgs", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut replies = Vec::with_capacity(100);
                for _ in 0..100 {
                    let (reply_tx, reply_rx) = oneshot::channel();
                    // Unknown message shape: cmd_console::dispatch returns
                    // {"type":"error","reason":"parse",...} via its initial
                    // serde_json::from_value guard — fast path, no DB write,
                    // no broadcast. We're measuring the mpsc → parse → oneshot
                    // round-trip.
                    let msg = serde_json::json!({"type":"bench_noop"});
                    handle
                        .tx
                        .send(loop_::Cmd::HandleConsoleMsg { msg, reply: reply_tx })
                        .await
                        .unwrap();
                    replies.push(reply_rx);
                }
                for rx in replies {
                    rx.await.unwrap();
                }
            });
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(3)).sample_size(10);
    targets = bench_tick_latency, bench_console_dispatch_throughput
}
criterion_main!(benches);
```

- [ ] **Step 2: Verify it compiles**

Run from the repo root:

```bash
cargo build --benches -p cliptown-world
```

Expected: clean build, no errors. If `cliptown_world::loop_::Handle.view_rx` / `.tx` aren't `pub`, the build will tell you; both are pub (see `crates/world/src/loop_.rs:58-63`).

- [ ] **Step 3: Smoke-run the new benches**

Run from the repo root:

```bash
cargo bench -p cliptown-world 2>&1 | tail -40
```

Expected: both benches produce criterion output with a `time:` line. Don't worry about the absolute numbers yet; you're just confirming the benches execute without panicking. Per-bench duration is ~3s (criterion's `measurement_time`); total run ~10-15s.

If a bench panics (e.g., `view_rx.changed().await.unwrap()` panics with "channel closed"), the spawned loop dropped its view sender — that means the loop task exited unexpectedly. Investigate by checking that `_dir` and `handle` are both held alive across iters.

- [ ] **Step 4: Don't commit yet** — the bench rewrite + check.mjs + baselines all land in one commit at the end of Task 2.

---

## Task 2: Update `bench/check.mjs` extract recipe

**Files:**
- Modify: `bench/check.mjs:23-31`

- [ ] **Step 1: Apply the extract recipe change**

In `bench/check.mjs`, find the `applyExtract` function (around lines 22-32):

```js
function applyExtract(extract, medianNs) {
  switch (extract) {
    case "median_ns_to_us":
      return medianNs / 1_000;
    case "1000_div_median_us":
      // mpsc bench measures 1000 msgs at a time; throughput = 1000 / sec.
      return 1_000 / (medianNs / 1_000 / 1_000_000);
    default:
      throw new Error(`unknown extract recipe: ${extract}`);
  }
}
```

Replace the `case "1000_div_median_us":` arm with `case "100_div_median_us":` and update the divisor + comment:

```js
function applyExtract(extract, medianNs) {
  switch (extract) {
    case "median_ns_to_us":
      return medianNs / 1_000;
    case "100_div_median_us":
      // console_dispatch_throughput bench fires 100 msgs per iter;
      // throughput = 100 msgs / (median_ns → sec).
      return 100 / (medianNs / 1_000 / 1_000_000);
    default:
      throw new Error(`unknown extract recipe: ${extract}`);
  }
}
```

That's the only change in this file.

- [ ] **Step 2: Verify it still parses**

Run:

```bash
node -e "import('./bench/check.mjs').catch(e => { console.error(e); process.exit(1); })"
```

Expected: no syntax errors. (The script will probably error out somewhere reading baselines, but it will at least load — that's all this step verifies.)

- [ ] **Step 3: Update `bench/baselines.json` to point at new benches with placeholder values**

Replace the entire file with:

```json
{
  "_comment": "Phase 1 baselines. Re-captured 2026-05-12 against M11 real bench harness on an Apple Silicon dev box. Re-baseline (delete this file's numbers, run `cargo bench`, copy criterion medians here per the extract recipes in bench/check.mjs) whenever the bench bodies in `crates/world/benches/world_bench.rs` materially change.",
  "version": 2,
  "tolerance_pct": 20,
  "metrics": {
    "world.tick_latency_us": {
      "baseline": 0,
      "unit": "us",
      "criterion_bench": "tick_latency_real_loop",
      "extract": "median_ns_to_us"
    },
    "world.console_dispatch_throughput_msgs_per_sec": {
      "baseline": 0,
      "unit": "msgs/s",
      "criterion_bench": "console_dispatch_throughput_100_msgs",
      "extract": "100_div_median_us"
    },
    "frontend.console_fcp_ms": {
      "baseline": null,
      "unit": "ms",
      "ceiling": 300,
      "_note": "Asserted directly by packages/frontend/bench/fcp.spec.ts."
    },
    "frontend.town_fcp_ms": {
      "baseline": null,
      "unit": "ms",
      "ceiling": 500,
      "_note": "Asserted directly by packages/frontend/bench/fcp.spec.ts."
    }
  }
}
```

Both backend baselines are placeholders (`0`) at this step; Task 3 fills them in with real measurements.

- [ ] **Step 4: Commit the bench rewrite + check.mjs + placeholder baselines together**

```bash
git add crates/world/benches/world_bench.rs bench/check.mjs bench/baselines.json
git commit -m "$(cat <<'EOF'
feat(bench): real loop_::spawn-driven harness

tick_latency_real_loop times one Cmd::Tick round-trip through the
spawned world loop (move_sys::step_all + scheduler::tick +
proximity::compute_and_emit + view_tx.send). The watch::Receiver is
cloned per iter so .changed() actually waits for the next tick.

console_dispatch_throughput_100_msgs fires 100 Cmd::HandleConsoleMsg
with oneshot replies. The dispatcher's serde parse-error early return
gives a fast reply without DB writes or broadcast — we measure the
mpsc → parse → oneshot round-trip, the same lane every real console
command travels for its initial hop.

bench/check.mjs swaps the 1000_div_median_us extract for 100_div_
median_us. bench/baselines.json renames the throughput key + points at
the new criterion bench names; backend values are placeholder zeros
that get filled in by the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Capture fresh baselines

**Files:**
- Modify: `bench/baselines.json` (values only — the `0` placeholders from Task 2)

- [ ] **Step 1: Run criterion + capture medians**

Run from the repo root:

```bash
cargo bench -p cliptown-world 2>&1 | tee /tmp/world-bench.log
```

Expected: both benches produce output. The full run takes ~10-15 seconds.

After the run, fetch the medians from criterion's per-bench estimates JSON:

```bash
TICK_NS=$(jq '.median.point_estimate' target/criterion/tick_latency_real_loop/new/estimates.json)
DISPATCH_NS=$(jq '.median.point_estimate' target/criterion/console_dispatch_throughput_100_msgs/new/estimates.json)
echo "tick_ns=$TICK_NS"
echo "dispatch_ns=$DISPATCH_NS"
```

Expected: both variables hold a positive nanosecond integer.

- [ ] **Step 2: Convert to baseline units**

The extract recipes in `bench/check.mjs` turn nanosecond medians into baseline-unit values:

- `world.tick_latency_us` uses `median_ns_to_us` → `medianNs / 1_000`
- `world.console_dispatch_throughput_msgs_per_sec` uses `100_div_median_us` → `100 / (medianNs / 1_000 / 1_000_000)` = `100_000_000_000 / medianNs`

Compute both:

```bash
node -e "
const tickNs = $TICK_NS;
const dispatchNs = $DISPATCH_NS;
console.log('tick_latency_us:', (tickNs / 1_000).toFixed(3));
console.log('dispatch_throughput_msgs_per_sec:', Math.round(100_000_000_000 / dispatchNs));
"
```

Note the two values.

- [ ] **Step 3: Update `bench/baselines.json` with the captured values**

Open `bench/baselines.json` and replace the two `"baseline": 0` entries with the numbers from Step 2. Round `tick_latency_us` to 3 decimal places; throughput should be an integer (msgs/sec).

- [ ] **Step 4: Verify the gate is happy**

Run from the repo root:

```bash
node bench/check.mjs
```

Expected output (formatted; values will match the dev box):

```json
{
  "ok": true,
  "tolerance_pct": 20,
  "results": [
    {"name": "world.tick_latency_us", "status": "ok", "delta_pct": 0, ...},
    {"name": "world.console_dispatch_throughput_msgs_per_sec", "status": "ok", "delta_pct": 0, ...},
    {"name": "frontend.console_fcp_ms", "status": "skipped", ...},
    {"name": "frontend.town_fcp_ms", "status": "skipped", ...}
  ]
}
```

Exit code must be 0. `delta_pct` should be 0.00 or close to it (you just set the baselines to the measured values).

- [ ] **Step 5: Commit**

```bash
git add bench/baselines.json
git commit -m "$(cat <<'EOF'
chore(bench): capture M11 baselines on dev box

Replaces the placeholder zeros from the prior commit with real
medians captured via `cargo bench -p cliptown-world` on Apple Silicon.
`node bench/check.mjs` exits 0 with delta_pct ≈ 0 against these
values. CI on ubuntu-latest will produce different absolute numbers
but the +/-20% tolerance lets the gate ride through normal variance.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: CHANGELOG + TODOS

**Files:**
- Modify: `CHANGELOG.md` (top of file; also one bullet retirement in Phase-1 known limitations)
- Modify: `TODOS.md` (one new Completed entry)

This task also folds in the small stale-bullet cleanup that was deferred from the M11 hook-bridge PR — the Phase-1 known limitations list mentions "criterion benches are still placeholders". Replace that bullet rather than leaving it stale.

- [ ] **Step 1: Insert a new section at the top of `CHANGELOG.md`**

Find the `## M11 — hook bridge parity (2026-05-12)` heading near the top of the file. Insert this NEW section ABOVE it (so the latest entry is first):

```markdown
## M11 — real bench harness (2026-05-12)

Replaces the two placeholder benches in `crates/world/benches/world_bench.rs`
with measurements that drive a real `loop_::spawn` world handle:

- **`tick_latency_real_loop`** times one `Cmd::Tick` round-trip
  (`move_sys::step_all` + `scheduler::tick` +
  `proximity::compute_and_emit` + `view_tx.send`). The watch receiver
  is cloned per iter so `.changed()` actually waits for the next tick.
- **`console_dispatch_throughput_100_msgs`** fires 100
  `Cmd::HandleConsoleMsg` with oneshot replies. The dispatcher's
  `serde_json::from_value` parse-error early return gives a fast
  reply without DB writes or broadcast — measures the
  mpsc → parse → oneshot round-trip.

`bench/check.mjs` swaps the `1000_div_median_us` extract recipe for
`100_div_median_us`. `bench/baselines.json` carries fresh
dev-box-captured numbers and renames the throughput key to
`world.console_dispatch_throughput_msgs_per_sec`. CI gate stays
`continue-on-error: true` until more ubuntu-latest samples land —
that flip is a separate follow-up.

```

- [ ] **Step 2: Retire the stale placeholder-benches bullet in CHANGELOG**

Find the `### Known limitations carried into Phase 1` section. The bullet currently reads roughly:

```markdown
- Two criterion benches (`cargo bench -p cliptown-world`) are still
  placeholders: tick latency = sum 0..1000, throughput = in-process
  1k-msg mpsc. Phase 1 swaps in real `loop_::spawn`-driven harnesses.
```

Replace that bullet with:

```markdown
- Criterion benches: closed under M11 real bench harness (this section).
```

Leave the `bench.yml` `continue-on-error: true` bullet directly above it
in place — that's a separate gate-flip follow-up.

- [ ] **Step 3: Add a Completed entry to `TODOS.md`**

Open `TODOS.md`. Under the `## Completed` heading, ABOVE the existing `### M11 hook bridge parity` entry, insert:

```markdown
### M11 real bench harness — 2026-05-12
**Source:** Phase 1 known-limitation cleanup. PR `<TBD — fill in at PR creation>`.

Was: `crates/world/benches/world_bench.rs` shipped Phase 0 with two
placeholder benches — `tick_latency_per_loop_iter` ran `sum 0..1000`
inside a tokio runtime; `mpsc_throughput_1k_msgs` measured a generic
in-process mpsc channel. Neither touched real world code.

Fixed: both benches now drive a real `loop_::spawn` handle.
`tick_latency_real_loop` measures one `Cmd::Tick` round-trip end to
end; `console_dispatch_throughput_100_msgs` fires 100
`Cmd::HandleConsoleMsg` through the same dispatcher lane real console
commands use. `bench/check.mjs` learned the `100_div_median_us` extract
recipe; `bench/baselines.json` carries fresh medians captured on the
dev box. The Phase-1 known-limitations bullet about placeholder
benches retires.

```

You will fill in the `<TBD — fill in at PR creation>` placeholder after
`gh pr create` returns the PR number. (This is the only TBD anywhere in
the plan and it exists by design — the PR number doesn't exist until
the PR is created.)

- [ ] **Step 4: Commit**

```bash
git add CHANGELOG.md TODOS.md
git commit -m "$(cat <<'EOF'
docs: changelog + TODOS for M11 real bench harness

Add the M11 real-bench-harness section atop CHANGELOG (alongside the
M11 hook-bridge entry that shipped earlier today). Retire the stale
'criterion benches are placeholders' bullet from the Phase-1 known
limitations. TODOS gets the matching Completed entry.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Verification sweep

**Files:** none — read-only.

- [ ] **Step 1: Confirm cargo tests still pass**

Run from the repo root:

```bash
cargo test -p cliptown-world 2>&1 | grep "test result:" | awk '{sum += $4} END {print "Total Rust tests passed:", sum}'
```

Expected: `Total Rust tests passed: 219` (unchanged — this PR doesn't touch library code).

- [ ] **Step 2: Confirm benches still compile + run**

```bash
cargo build --benches -p cliptown-world
```

Expected: clean build.

- [ ] **Step 3: Confirm the bench gate exits 0**

```bash
node bench/check.mjs
```

Expected: exit 0, both `world.*` metrics show `status: "ok"`, both `frontend.*` skipped.

- [ ] **Step 4: Confirm worker / adapter / frontend tests still pass**

```bash
pnpm -F @cliptown/worker test
pnpm -F @cliptown/adapter-core test
pnpm -F @cliptown/adapter-claude-code test
pnpm -F @cliptown/adapter-codex test
pnpm -F @cliptown/adapter-opencode test
```

Expected: all green; counts unchanged from the M11 hook-bridge baseline (`worker` 65, `adapter-core` 3, `adapter-claude-code` 8, `adapter-codex` 12, `adapter-opencode` 12).

No commit in this task — it's pure verification.

---

## Definition of done

- `crates/world/benches/world_bench.rs` body rewritten; placeholder bench
  patterns (sum 0..1000, in-process 1k-msg mpsc) gone.
- `bench/check.mjs` has the `100_div_median_us` extract recipe; the old
  `1000_div_median_us` arm is removed.
- `bench/baselines.json` has both backend metrics filled in with
  fresh dev-box medians and the throughput key renamed to
  `world.console_dispatch_throughput_msgs_per_sec`.
- `cargo bench -p cliptown-world` runs without panics.
- `node bench/check.mjs` exits 0 with `delta_pct ≈ 0`.
- `cargo test -p cliptown-world` still 219 green.
- CHANGELOG carries the new `## M11 — real bench harness` section and
  retires the stale placeholder-benches bullet.
- TODOS.md Completed has the matching entry (PR number to be filled at
  PR-creation time).
