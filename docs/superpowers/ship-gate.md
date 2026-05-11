# Phase 0 Ship Gate

Cliptown's ship gate verifies the 9 spec invariants. Phase 0 ships rust-layer
proofs for each invariant; Phase 1 lifts each to a Playwright UI proof.

## Invariant cross-reference

| # | Invariant | Plan task | Rust proof | UI proof (Phase 1) |
|---|-----------|-----------|------------|--------------------|
| 1 | All 3 adapters present + connected | M9.1 | `packages/adapters/{claude-code,codex,opencode}/test/hooks.test.ts` (shape) + `packages/worker/test/contract.test.ts` (cross-adapter) | `packages/frontend/e2e/ship-gate.spec.ts` § 11.1 |
| 2 | Operator directive → manager subtask → engineer assignment | M9.2 / M5.3 | `crates/world/tests/e2e_directive_chain.rs` | `packages/frontend/e2e/ship-gate.spec.ts` § 11.2 |
| 3 | Engineer walks to required_room (A* respects doors) | M9.3 / M1.13 | `crates/world/tests/movement.rs` (cross-room walk) | `packages/frontend/e2e/ship-gate.spec.ts` § 11.3 (sprite animation) |
| 4 | Artifact lands at exact canonical path | M9.4 / M5.4 | `crates/world/tests/e2e_engineer_artifact.rs` | `packages/frontend/e2e/ship-gate.spec.ts` § 11.4 (operator console surface) |
| 5 | Epistemic discipline (hypothesis_state → test_record → resolve) | M9.5 / M5.4 | `crates/world/tests/e2e_engineer_artifact.rs` (asserts 3 epistemic_log entries) | _n/a — DB invariant_ |
| 6 | Review cycle (round++ + max-rounds escalation) | M9.6 / M5.6 | `crates/world/tests/e2e_review_cycle.rs` | `packages/frontend/e2e/ship-gate.spec.ts` § 11.6 |
| 7 | Cross-startup chat in cafe (public room) | M9.7 / M7.2 | `crates/world/tests/e2e_cafe.rs` (routing) | `packages/frontend/e2e/ship-gate.spec.ts` § 11.7 (rendering) |
| 8 | Multi-tenant isolation | M9.8 / M6.1 | `crates/world/tests/e2e_isolation.rs` (delivery) | `packages/frontend/e2e/ship-gate.spec.ts` § 11.8 (operator view) |
| 9 | All 3 adapters complete a task end-to-end | M9.9 / M8.3 | `packages/worker/test/contract.test.ts` (cross-adapter shape) | `scripts/smoke-real-llm.sh` + `pnpm -F @cliptown/e2e-real-llm start` — claude_code verified 2026-05-11 (claude-code CLI 2.1.138, haiku → `awaiting_review`, canonical artifact_path); codex/opencode pending A2-equivalent worker wiring |

## Running the ship gate

Rust layer:
```bash
cargo test -p cliptown-world
```
180 tests pass — all 9 invariants asserted.

UI layer:
```bash
cd packages/frontend
pnpm e2e:install   # one-time
pnpm e2e
```
14 tests pass: 3 smoke (redirect, status indicator, town back link), 2 keymap regression (input suppression, Esc-dismiss), 7 ship-gate invariants (§ 11.1, § 11.2, § 11.3, § 11.4, § 11.6, § 11.7, § 11.8), plus two card-rendering tests that back § 11.6 (the review-round badge and the `changes_requested → in_progress` coalesce). #5 stays rust-only (DB invariant); #9 lifts to the maintainer-run real-LLM runners described below — not part of the Playwright suite because it costs Anthropic API tokens to exercise.

UI proofs that need synthetic ConsoleOutbound frames (§ 11.2, § 11.3, § 11.4, § 11.6, § 11.7, § 11.8) use dev-only test hooks, all `import.meta.env.DEV`-gated and tree-shaken from production builds:
- `__cliptownDispatch(msg)` (store.ts) — push a frame through the reducer
- `__cliptownStopWS()` (store.ts) — disconnect the live WS so synthetic state doesn't race
- `__cliptownInspectAvatarSprite(id)` (PixiStage.tsx) — read sprite x/y/alpha for canvas-rendered assertions

## Real-LLM run (M9.10)

cliptown is an open-source project; we deliberately do not require a private
Anthropic API key to live in CI secrets, so the real-LLM E2E is not wired as
a GitHub Actions workflow. The maintainer runs it locally instead:

- `scripts/smoke-real-llm.sh` — bash, human-debuggable, colored output.
- `pnpm -F @cliptown/e2e-real-llm start` — TypeScript runner that emits a
  single JSON summary on stdout (the proof artifact for § 11.9).

Budget is capped at `$0.50/run` via `BUDGET_CAP_USD` / `E2E_BUDGET_CAP_USD`;
breach fails the run. Triggered manually by maintainers.

### Latest verified pass

```text
date: 2026-05-11
claude-code CLI: 2.1.138 (OAuth)
backend: claude_code
duration: ~32s
{
  "ok": true,
  "task_status": "awaiting_review",
  "artifact_path": "workspaces/<sid>/artifacts/smoke-haiku.md",
  "artifact_bytes": 71,
  "budget_spent_usd": 0
}
```

`budget_spent_usd=0` is a Phase 1 limitation, not a billing claim: the
claude-code CLI under OAuth does not push token usage into the worker's
hook stream, so the world's `report_budget` ladder never fires for these
runs. The world-side cap is still enforced if/when a report arrives.

### Scope of the verified pass

Only `claude_code` is exercised end-to-end. `codex` and `opencode` share
the MCP HTTP adapter contract (A1') and contract-test shape coverage but
their worker `--real` path throws `not_yet_supported_in_real_mode` until
each gets its own A2-equivalent prompt/CLI wiring (tracked as M9.10
follow-up, not blocking § 11.9 close because the invariant is a "task
completes" proof — claude_code completing the haiku and landing the
artifact at the canonical path proves the chain works).
