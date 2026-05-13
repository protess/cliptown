# cliptown

cliptown is a multi-startup AI agent simulator with a 2D operator console.
Real LLM agents (Claude Code, codex, opencode) run as workers in the same
"town", each working on tasks for their own startup. The operator watches
from a god-view and can possess any town to drop into the action.

## Status

**Phase 0 — sealed (2026-05-11).** All 9 spec invariants pass at the rust
layer, the real-LLM E2E (§ 11.9) is self-implemented end-to-end against a
real `claude` CLI, and a 20%-tolerance perf regression gate runs on every
PR. See [`CHANGELOG.md`](CHANGELOG.md) for the full Phase 0 highlight
reel; see [`docs/superpowers/ship-gate.md`](docs/superpowers/ship-gate.md)
for the invariant matrix.

Test counts at seal: 213 rust, 62 worker, 12 adapter, 14 frontend Playwright.

## Quickstart

Prereqs: Rust 1.86, Node 20, pnpm 9.

```bash
# Install deps + build cross-language schema.
pnpm install

# Build the world server (Rust).
cargo build --workspace

# Build the worker + adapters (TypeScript).
pnpm -r build

# Run tests.
cargo test --workspace
pnpm -r test
```

To run the operator console locally:

```bash
# Terminal 1: world server (binds 127.0.0.1:8080).
cargo run -p cliptown-world

# Terminal 2: frontend dev server (Vite, 127.0.0.1:5173).
pnpm --filter @cliptown/frontend dev
```

Open `http://127.0.0.1:5173/` — redirects to `/console`.

## Deploy

For Docker / Fly.io / cloud deployment, see [`docs/DEPLOY.md`](docs/DEPLOY.md).
Quick local-prod equivalent:

```bash
docker compose up -d
curl http://localhost:8080/health
```

## Architecture

- `crates/world/` — Rust world server (single-thread mpsc inbox, SQLite WAL state, axum WS).
- `packages/worker/` — TypeScript agent worker (WS to world, MCP proxy, supervisor).
- `packages/adapters/{claude-code,codex,opencode,core}/` — backend CLI adapters + shared contract.
- `packages/frontend/` — React + Pixi.js console + 2D town view.
- `packages/protocol/` — auto-generated TS types from Rust ts-rs exports.

See `docs/superpowers/specs/2026-05-07-cliptown-design.md` for the full design.

## Spec invariants

The 9 ship-gate invariants are documented at `docs/superpowers/ship-gate.md`
with cross-references to existing rust tests.

## Contributing

See `CONTRIBUTING.md`.
