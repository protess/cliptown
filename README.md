# cliptown

cliptown is a multi-startup AI agent simulator with a 2D operator
console. Real LLM agents (Claude Code, codex, opencode) run as workers
in the same "town", each working on tasks for their own startup. The
operator watches from a god-view and can possess any town to drop into
the action.

## Status

**Phase 0–2 sealed; Phase 3 underway.** All 9 spec invariants pass at
the rust layer. Real-LLM § 11.9 ship-gate verified against
claude-code, codex, and opencode. Phase 2 shipped multica patterns
(daemon health buckets, per-task execenv directories, skills MVP +
operator UI). Phase 3 added local-first deploy (with local-LLM
routing) plus cloud (Fly.io) for shared instances.

Test counts on main (as of M13 Theme A):

| Layer | Tests |
|---|---|
| `cargo test -p cliptown-world` | 246 |
| `pnpm -F @cliptown/worker test` | 75 |
| `pnpm -F @cliptown/adapter-{core,claude-code,codex,opencode} test` | 35 |
| `pnpm -F @cliptown/frontend e2e` | 16 |
| `node bench/check.mjs` | ok |

See [`CHANGELOG.md`](CHANGELOG.md) for the full milestone reel.

## Quickstart

Prereqs: Rust 1.86, Node 20, pnpm 9. (Or just Docker — skip ahead to
[Deploy](#deploy).)

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

Open `http://127.0.0.1:5173/` — redirects to `/console`. The default
operator token is `dev-token`; the UI uses it automatically in dev mode.

## Real-LLM smoke

The § 11.9 smoke spins up a world, creates a startup + engineer agent,
and drives one task end-to-end through a real CLI:

```bash
# Defaults to claude_code. Set BACKEND=codex|opencode to swap.
BACKEND=claude_code BUDGET_CAP_USD=1.0 \
  bash scripts/smoke-real-llm.sh
```

Costs ~$0.15–0.35 per run depending on backend. Each spawn lands a
markdown artifact at `workspaces/<sid>/artifacts/<tid>.md` and flips
the task to `awaiting_review`.

## Deploy

cliptown defaults to running **locally** — the most interesting LLM
backends are local (ollama, llama.cpp, vLLM), and a cloud VM can't
reach your GPU. Cloud (Fly.io) is for sharing a hosted instance with
collaborators against a hosted provider API.

```bash
# Native (fastest dev loop):
pnpm dev

# Or same-as-prod image on your laptop:
docker compose up -d
curl http://localhost:8080/health
```

Local LLM via ollama, Fly.io, and other targets — see
[`docs/DEPLOY.md`](docs/DEPLOY.md).

## Observability

The world exposes a Prometheus text-exposition endpoint at
`http://localhost:8080/metrics`. The repo ships:

- [`docs/observability/grafana/cliptown-overview.json`](docs/observability/grafana/cliptown-overview.json)
  — importable Grafana dashboard with panels for tick rate,
  MCP throughput, task distribution, agent health, and
  per-startup budget %.
- [`docs/observability/alerts/cliptown.yml`](docs/observability/alerts/cliptown.yml)
  — Prometheus alert rules for tick stall, MCP error rate,
  budget warning, and agent lost/offline.

Wire-up: P5 Theme E (docker-compose) provides the
`observability` profile that pre-provisions a Prometheus +
Grafana pair with these files mounted. Until that lands,
import the dashboard JSON manually and add the alerts to
your existing Prometheus config.

## Where things live

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — system layout, MCP tools,
  data flow.
- [`docs/OPERATOR.md`](docs/OPERATOR.md) — how operators use the
  console: possessing, managing skills, reviewing tasks, budgets.
- [`docs/AGENT.md`](docs/AGENT.md) — what cliptown looks like from an
  adapter-spawned CLI's POV: workdir layout, skills, MCP surface,
  CLAUDE.md contract.
- [`docs/DEPLOY.md`](docs/DEPLOY.md) — local-first deploy, local LLM
  via ollama, docker-compose, Fly.io, secrets pattern.
- [`docs/superpowers/specs/`](docs/superpowers/specs/) — per-milestone
  design specs (chronological).
- [`docs/superpowers/plans/`](docs/superpowers/plans/) — per-milestone
  implementation plans (chronological).
- [`docs/superpowers/ship-gate.md`](docs/superpowers/ship-gate.md) —
  the 9 spec invariants + cross-refs to rust tests.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md).
