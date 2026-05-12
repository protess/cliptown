# Changelog

## M11 — hook bridge parity (2026-05-12)

`codex` and `opencode` adapters now actually flow hook events. Both
previously advertised `[session_stop, session_error]` but never POSTed
anything to the HTTP hook bridge they stood up — dead code.

- **codex**: in-adapter streaming JSONL parser
  (`packages/adapters/codex/src/event_parser.ts`) converts
  `item.started` / `item.completed` for `command_execution` (tool name
  normalized to `"shell"`) and `mcp_tool_call` (tool name from
  `item.tool`) into pre_tool/post_tool HookEvents. Process exit emits
  session_stop on 0 or session_error with stderr_tail on non-zero.

- **opencode**: rebuilt around `opencode serve --port 0 --pure` +
  REST/SSE. The adapter spawns a headless opencode server, subscribes to
  `/event` SSE, and maps `message.part.updated` frames to HookEvents
  via `event_mapper.ts` — with proper per-callID dedup so pre_tool fires
  once (first `running` transition) and post_tool fires once (first
  terminal status). `session.idle` is the terminal signal. `opencode run
  --format json` is abandoned because it only emits already-completed
  tool frames.

- **HTTP hook bridge** is removed from codex/opencode (was dead). Still
  used by claude-code where the CLI's `settings.json` hook script
  contract genuinely needs it.

- **claude-code**: a smoke-driven fix — claude CLI 2.1.x ignores the
  `CLAUDE_CODE_SETTINGS` env var; the adapter now also passes
  `--settings <path>` so PreToolUse/PostToolUse/Stop hooks fire. The
  shared bridge also learned to read `tool_name` from claude payloads
  (it only checked `tool` before, so HookEvent.tool came out empty).

- Capability advertising now matches reality:
  `claude_code`/`codex`/`opencode` all carry
  `[pre_tool, post_tool, session_stop, session_error]`.

§ 11.9 smoke verified across all three backends — worker.log shows
named tools (`Write`, `ToolSearch`, `mcp__cliptown__task_done` for
claude; `shell`, `task_done` for codex; `apply_patch`,
`cliptown_task_done` for opencode) and a closing `session_stop`.

## Phase 0 — bring-up complete (2026-05-11)

Phase 0 closes with all 9 spec invariants proven at the Rust layer and the
real-LLM ship gate (§ 11.9) self-implemented end-to-end against a real
`claude` CLI. Phase 1 begins with the Phase 2 backlog from
`docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md`.

### What ships in Phase 0

- **World server** (`crates/world/`): SQLite-backed mpsc-routed loop with
  16 MCP tools, axum HTTP + WS surface, room A\* pathfinding, task
  scheduler, budget ladder (warn/95/pause), agent supervisor, sandbox
  path resolver.
- **Worker** (`packages/worker/`): WS client, MCP correlation, mock and
  real-LLM run modes, supervisor-respawn semantics.
- **Adapters** (`packages/adapters/{claude-code,codex,opencode}/`): unified
  `BackendAdapter` contract with hook bridge, MCP-at-the-world HTTP wiring,
  contract-tested cross-adapter.
- **Frontend** (`packages/frontend/`): React + Pixi 2D console, kanban,
  chat panel, possess transition, system event feed, 14 Playwright tests.

### Test counts at seal

| Layer | Tests |
|---|---|
| `cargo test -p cliptown-world` | 213 |
| `pnpm -F @cliptown/worker test` | 62 |
| `pnpm -F @cliptown/adapter-{core,claude-code,codex,opencode} test` | 12 |
| `pnpm -F @cliptown/frontend test` (Playwright) | 14 |

### M9.10 — real-LLM E2E (§ 11.9)

The longest milestone of Phase 0. Closed in 9 PRs across two arcs:

**Architecture (one merge train, #19 → #25):**
- `A1'` MCP-over-HTTP at the world (route + 16-tool catalog + bearer auth)
- `A2` worker `--real` flag spawns the adapter against world MCP
- `A3` `scripts/smoke-real-llm.sh` runs the chain locally with colored output
- `B` `pnpm -F @cliptown/e2e-real-llm start` runs it with structured JSON
- `C` was shipped then reverted in #25 — cliptown is open source and we
  don't want fork contributors to need an Anthropic API key in CI secrets

**Hardening (#26 → #29):**
- `#26` First real run surfaced three smoke-execution bugs (pnpm
  workspace discovery, script arg re-shelling, world's per-agent random
  secret unreachable from out-of-band workers). All fixed; smoke passes
  green against `claude` 2.1.138 OAuth, haiku-shaped output at canonical
  `workspaces/<sid>/artifacts/<tid>.md` path, task state machine
  transitions to `awaiting_review`.
- `#27` Ship-gate § 11.9 cell lifts from "real-LLM only — M9.10"
  placeholder to a runner pointer + verified-pass timestamp.
- `#28` `CLIPTOWN_TEST_DISABLE_SUPERVISOR=1` silences nine
  `spawn_agent failed` warnings per smoke run.
- `#29` Budget telemetry: `claude --output-format json` lets the adapter
  scrape `total_cost_usd`; worker forwards as `report_budget` WS frame;
  world's `apply_report` uses CLI cost when present, falls back to the
  pricing table otherwise. Smoke output now shows
  `budget_spent_usd ≈ $0.31` instead of `$0` — the cap (default $0.50)
  has real teeth.
- `#31` codex + opencode adapters lifted to A2-equivalent. `codex exec
  --json` + `-c mcp_servers.cliptown.{url,bearer_token_env_var}` +
  `--dangerously-bypass-approvals-and-sandbox` (full-auto skips shell
  but not MCP). `opencode run --format json --pure --dir <cwd>
  --dangerously-skip-permissions` with per-spawn `opencode.json` in the
  workspace. `pickAdapter` drops the `not_yet_supported_in_real_mode`
  throws. Bundled with a `fix(m9.10)` commit for the MCP HTTP
  notifications response: claude tolerated `{}` 200; rmcp 0.6+ used by
  codex rejected it. Spec-compliant 202 + empty body now.

### Phase 0 hardening (TODOS.md → Completed)

- **P3 emit_system_event JSON fallback**: malformed payloads now broadcast
  the raw string instead of silently degrading to `Value::Null`, so the
  audit log and operator console stay in sync. Loud-fail with
  `tracing::error!`.

### M10.1 — performance regression gate

- `cargo bench -p cliptown-world` runs two criterion benches (placeholder
  tick latency + mpsc throughput) and writes JSON estimates to
  `target/criterion/`.
- `bench/check.mjs` reads criterion's medians, converts to baseline units,
  and fails (exit 1) if any metric regresses by more than `tolerance_pct`
  (20% default).
- `.github/workflows/bench.yml` runs the gate on every PR. Phase 0 ships
  the gate as `continue-on-error: true` while the CI baselines stabilize
  vs developer hardware; flip to a hard gate after a few CI samples.
- `pnpm -F @cliptown/frontend bench:fcp` (added 2026-05-11) runs the
  Playwright FCP bench against a production `vite build`. Ceilings: 300ms
  for `/console`, 500ms for `/town/:id`. Asserts directly in the test, so
  failure surfaces as a Playwright test failure (no separate comparator
  needed for the frontend metrics). Local measurements at seal: /console
  ≈ 150ms, /town/:id ≈ 320ms — both well under their ceilings.

### Known limitations carried into Phase 1

- (No major known limitations carry forward from Phase 0; all adapter
  budget tracking + hook flow now closed under M11.)
- Two criterion benches are still placeholders (sum 0..1000 for "tick
  latency", in-process 1k-msg mpsc for "throughput"). Phase 1 swaps in
  real `loop_::spawn`-driven harnesses.
- Frontend FCP bench (`packages/frontend/bench/fcp.spec.ts`) is
  `test.describe.skip`; CI gate covers it via the ceiling-only check
  once the Playwright run includes it.

### Phase 2 backlog (from M9.10 spec)

Three patterns imported from multica that didn't block § 11.9:

- **P2.1 — Daemon health buckets**: 4-state worker liveness
  (online/recently_lost/offline/about_to_gc) so transient network blips
  don't surface to the operator console as crashes. ~2-3h.
- **P2.2 — Skills system**: reusable per-task capabilities stored as
  markdown + files, attached to agents, written into the per-task
  workdir at spawn. ~1-2 weeks. Depends on P2.3.
- **P2.3 — Per-task execenv directories**: `{workspaces}/{sid}/{tid}/`
  with workdir + injected CLAUDE.md + skill files + 7-day GC. ~3-5 days.
  Prerequisite for P2.2.
