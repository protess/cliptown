# Phase 0 Ship Gate

Cliptown's ship gate verifies the 9 spec invariants. Phase 0 ships rust-layer
proofs for each invariant; Playwright UI proofs are scaffolded but deferred
to Phase 1+.

## Invariant cross-reference

| # | Invariant | Plan task | Existing test |
|---|-----------|-----------|----------------|
| 1 | All 3 adapters present + connected | M9.1 | `packages/adapters/{claude-code,codex,opencode}/test/hooks.test.ts` (shape) + `packages/worker/test/contract.test.ts` (cross-adapter) |
| 2 | Operator directive → manager subtask → engineer assignment | M9.2 / M5.3 | `crates/world/tests/e2e_directive_chain.rs` |
| 3 | Engineer walks to required_room (A* respects doors) | M9.3 / M1.13 | `crates/world/tests/movement.rs` (cross-room walk) |
| 4 | Artifact lands at exact canonical path | M9.4 / M5.4 | `crates/world/tests/e2e_engineer_artifact.rs` |
| 5 | Epistemic discipline (hypothesis_state → test_record → resolve) | M9.5 / M5.4 | `crates/world/tests/e2e_engineer_artifact.rs` (asserts 3 epistemic_log entries) |
| 6 | Review cycle (round++ + max-rounds escalation) | M9.6 / M5.6 | `crates/world/tests/e2e_review_cycle.rs` |
| 7 | Cross-startup chat in cafe (public room) | M9.7 / M7.2 | `crates/world/tests/e2e_cafe.rs` |
| 8 | Multi-tenant isolation | M9.8 / M6.1 | `crates/world/tests/e2e_isolation.rs` |
| 9 | All 3 adapters complete a task end-to-end | M9.9 / M8.3 | `packages/worker/test/contract.test.ts` (cross-adapter shape) |

## Running the ship gate

Rust layer:
```bash
cargo test -p cliptown-world
```
180 tests pass — all 9 invariants asserted.

UI layer (Phase 0 skeleton):
```bash
cd packages/frontend
pnpm e2e:install   # one-time
pnpm e2e
```
3 smoke tests pass: redirect, status indicator, town back link. Full UI invariant proofs land in Phase 1.

## Real-LLM run (M9.10)

`.github/workflows/e2e-real-llm.yml` runs the real-LLM E2E on workflow_dispatch only. Budget capped at `$0.50/run` via `E2E_BUDGET_CAP_USD`; breach fails the run. Triggered manually by maintainers.
