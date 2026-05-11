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
| 9 | All 3 adapters complete a task end-to-end | M9.9 / M8.3 | `packages/worker/test/contract.test.ts` (cross-adapter shape) | `scripts/smoke-real-llm.sh BACKEND=<claude_code\|codex\|opencode>` + `pnpm -F @cliptown/e2e-real-llm start` — all 3 verified 2026-05-11: claude-code 2.1.138, codex-cli 0.124, opencode 1.4.3. Each drops a haiku at the canonical `workspaces/<sid>/artifacts/<tid>.md`, task → `awaiting_review`. See [Real-LLM run](#real-llm-run-m910) for the per-backend matrix. |

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

### Latest verified pass — all three adapters (2026-05-11)

| backend       | CLI version    | artifact_bytes | task_status       | cost reported |
|---------------|----------------|----------------|-------------------|---------------|
| `claude_code` | claude 2.1.138 | 78             | `awaiting_review` | $0.31 (authoritative — claude-code reports `total_cost_usd`) |
| `codex`       | codex 0.124    | 76             | `awaiting_review` | unset → world's pricing-table fallback ($0 for `codex-default`; see below) |
| `opencode`    | opencode 1.4.3 | 67             | `awaiting_review` | $0.0000 (opencode-reported, model `openai/gpt-5.4-mini` on this user's plan) |

Each row was produced by `BACKEND=<id> bash scripts/smoke-real-llm.sh`
with the canonical haiku artifact lifted from a fresh tmpdir. The JSON
runner (`pnpm -F @cliptown/e2e-real-llm start`) produces equivalent
output and is the machine-readable proof attached to this gate.

### Known cost-reporting limitations

- **codex** — the CLI's JSONL stream emits token counts but no dollar
  cost, and `codex-default` (our placeholder model_id when the resolved
  model isn't surfaced) isn't in the world's hardcoded
  `price_per_mtok` table. Budget accrues $0 for codex runs until either
  codex starts emitting a cost field or we pass `--model` explicitly and
  add the resolved key to the price table.
- **opencode** — reports a real cost field; for this user's
  `openai/gpt-5.4-mini` plan that value is `$0.0000`. Not a bug — the
  CLI's own number passes through verbatim to `startups.budget_spent_usd`.
- **claude_code** — the only backend that currently produces a non-zero
  authoritative cost (`total_cost_usd` from `--output-format json`).
