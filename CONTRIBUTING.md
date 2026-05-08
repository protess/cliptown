# Contributing to cliptown

## Branch flow

- `main` — protected; only ship-gate-passing commits.
- Feature branches off `main`.
- PRs require all rust + worker tests green + ship gate untouched.

## Test gates

Before any PR:

```bash
cargo test --workspace
pnpm -r test
```

The full ship gate (rust layer) lives in `docs/superpowers/ship-gate.md`.

## Code style

### Rust
- `rustfmt` defaults.
- `clippy --all-targets` clean (warnings allowed for now; errors block).
- All SQL parameterized.
- All task transitions through `task_sm::next` — no direct status writes.
- Single-thread invariant: every WS frame becomes a `Cmd` on the loop's mpsc inbox; handlers don't touch state directly.

### TypeScript
- `tsc -b` clean under strict mode.
- No new top-level dependencies without discussion.
- Tests via vitest; UI smoke via Playwright (`pnpm --filter @cliptown/frontend e2e`).

## Adding a new MCP tool

1. Add the variant to `crates/world/src/protocol/ws_messages.rs` (regenerates TS types via ts-rs).
2. Add the dispatch arm in `crates/world/src/mcp_dispatch.rs`.
3. Add a unit test in `crates/world/tests/mcp_handlers.rs`.
4. Add the proxy method in `packages/worker/src/mcp.ts`.
5. Add the proxy method test in `packages/worker/test/mcp_correlation.test.ts`.

## Adding a backend adapter

Implement `BackendAdapter` from `@cliptown/adapter-core` in a new
`packages/adapters/<name>/` package. The adapter spawns the CLI, wires
its MCP config to point at the worker's UNIX socket, and uses
`startHookBridge` from `@cliptown/adapter-core` to normalize hook events.

## Spec changes

The design spec at `docs/superpowers/specs/2026-05-07-cliptown-design.md`
is the source of truth. Material changes go through the design review
process documented in `docs/superpowers/plans/2026-05-07-cliptown-phase-0.md`.
