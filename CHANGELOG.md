# Changelog

## M13 — Phase 3 Theme C: per-task routing preferences (2026-05-13)

Fifth Phase 3 theme. Tasks were routed implicitly to whatever
backend/model was provisioned on the agent at startup; cheaper
models couldn't be opted into for trivial subtasks.

- **`crates/world/migrations/0004_task_routing_preferences.sql`**
  (new) — adds nullable `preferred_backend` + `preferred_model` to
  `tasks`. NULL falls back to the agent's provisioned default.
- **`WorkerOutbound::TaskAssigned`** (ts-rs auto-export) carries the
  two new optional fields; the scheduler reads them from the queued-
  task row and forwards them on dispatch.
- **`task_set_preference` MCP tool** (22 tools total now) — manager-
  or-assignee can set/clear either field. Both managers (budget POV)
  and assignees (load POV) are reasonable callers. Cross-startup
  blocked, stranger callers refused. Audit row appended +
  `task_routing_changed` system_event broadcast for operator audit.
- Worker-side adapter honoring is documented but not enforced — the
  field arrives on `task_assigned`; worker implementations decide
  whether to override the spawn arguments. Carry-forward note.

Cost variance telemetry (estimated-vs-actual emission to
system_events) deferred — needs estimate inputs first.

## M13 — Phase 3 Theme B: operator RBAC (2026-05-13)

Fourth Phase 3 theme. Console access is no longer a single shared
`CLIPTOWN_OPERATOR_TOKEN`. Each operator has a row in the new
`operators` table with one of three roles, and mutating console
commands check the role before touching SQL or the broadcast bus.

- **`crates/world/migrations/0003_operators.sql`** (new) — `operators`
  table `(id, name, token UNIQUE, role CHECK(viewer|manager|admin),
  created_at)`. Seeded with `op_default / dev-token / admin` so the
  existing dev workflow keeps working.
- **`crates/world/src/auth.rs`** — `validate_operator_token` returns
  a typed `OperatorIdentity { id, name, role }` instead of `()`. Table
  lookup first, with `CLIPTOWN_OPERATOR_TOKEN` env-var fallback that
  returns a synthetic admin so legacy deployments keep working. 4 new
  unit tests cover seeded admin / unknown / viewer + manager rows /
  role ordering.
- **`crates/world/src/loop_.rs`** — `Cmd::HandleConsoleMsg` carries
  the `OperatorIdentity` captured at WS-hello validation so the loop
  can hand it to `cmd_console::dispatch` per inbound frame.
- **`crates/world/src/cmd_console.rs`** — added per-arm role gate.
  Viewer ≥ : `Hello`, `OperatorPossess`, `OperatorUnpossess`,
  `OperatorMove`, `OperatorRecheckBackends` (read-ish, no SQL writes
  beyond avatar state). Manager ≥ : `OperatorDirective`,
  `OperatorAcceptProposal`, `OperatorRejectProposal`,
  `OperatorForceAccept`, `OperatorForceFail`, `SkillAttach`,
  `SkillDetach`. Forbidden returns `{"type":"error","reason":"forbidden"}`
  before any SQL or broadcast — symmetric with the cross-startup
  rejection pattern.
- **`crates/world/src/http.rs::handle_console`** captures the identity
  from `validate_operator_token` and forwards it on each inbound
  command.
- **Tests** — `console_cmds.rs` gains 3 RBAC integration tests
  (viewer rejected on force_accept, viewer can possess+move, manager
  can force_accept). All 5 existing `tests/*.rs` files updated to
  pass an admin identity via `OperatorIdentity::admin_for_tests()`.

Admin-only operator-management commands (provision/revoke operator
rows, role changes) deferred — schema is ready, surface comes when
multi-operator deploys actually arrive.

## M13 — Phase 3 Theme D: observability (2026-05-13)

Third Phase 3 theme. cliptown now exposes a Prometheus-style
`/metrics` endpoint so ops can scrape liveness and load.

- **`/metrics`** HTTP endpoint emits text exposition format. Metrics:
  - `cliptown_mcp_calls_total` (counter, all MCP tool call attempts)
  - `cliptown_mcp_errors_total` (counter, calls that returned mcp_error)
  - `cliptown_agents{health="..."}` (gauge, 4 labels for P2.1 health buckets)
  - `cliptown_startups_active` (gauge)
  - `cliptown_budget_spent_usd{startup_id="..."}` + `cliptown_budget_cap_usd{...}` (gauges)
  - `cliptown_tasks{status="..."}` (gauge, 8 status labels)
  - `cliptown_tick_seq` (counter, monotonic loop liveness)
- **`crates/world/src/metrics.rs`** (new) — hand-rolled text format
  renderer + atomic global counters. No external prometheus crate
  dependency. 2 unit tests cover zero-state rendering + counter
  increment.
- **`mcp_dispatch::dispatch`** increments `mcp_calls_total` per call
  and `mcp_errors_total` per error.

Scrape latency: O(active startups + tasks). Single-digit ms on
current scale; revisit caching if it climbs past 100ms.

Structured tracing spans through hot paths deferred — incremental
add as needed.

## M13 — Phase 3 Theme F: documentation pass (2026-05-13)

Second Phase 3 theme. cliptown's contributor and operator docs were
sparse. README focused on Phase 0 details that have since rotted; no
ARCHITECTURE, no OPERATOR, no AGENT guide.

- **`README.md`** rewritten — current status reflects sealed Phase
  0-2, Phase 3 underway. Test counts updated. New "Real-LLM smoke"
  section. "Where things live" index points at the new docs.
- **`ARCHITECTURE.md`** (new) — topology diagram, per-component
  walkthrough (world, worker, adapters, frontend, protocol), MCP
  tools catalog summary (21 tools across task lifecycle / knowledge
  / world interaction / skills), architectural invariants, spec
  cross-references.
- **`docs/OPERATOR.md`** (new) — operator console guide. Health
  buckets, possess flow, kanban actions (accept/reject proposal,
  force accept/fail, review rounds), skills management, directives,
  budget controls, system events, SQL spelunking recipes.
- **`docs/AGENT.md`** (new) — what cliptown looks like from inside
  an adapter-spawned CLI. Workdir layout, CLAUDE.md contract, all
  21 MCP tools categorized, hook events, budget ladder, sandboxing
  rules, common patterns (finish task / propose subtask / read peer
  artifact / author skill).

No code paths affected; all tests stay green.

## M13 — Phase 3 Theme A: production deploy story (2026-05-13)

First Phase 3 theme. cliptown now ships a Dockerfile, docker-compose
config, and a Fly.io app config + deploy guide so operators can run it
for real workloads.

- **`Dockerfile`** (new) — multi-stage build. Stage 1 builds the rust
  world binary (release); stage 2 installs the worker's pnpm deps
  (workers run via tsx, no transpile step); stage 3 is a debian-slim
  runtime carrying the world binary + node 20 + worker source +
  migrations + `cliptown.toml`. Runs as the unprivileged `cliptown`
  user. Healthcheck against `/health`.
- **`docker-compose.yml`** (new) — single-service local-prod equivalent
  with persistent volumes for `/data` (SQLite) and `/workspaces`
  (per-task execenv). Forwards provider keys from a gitignored `.env`.
- **`fly.toml`** (new) — single-VM single-region Fly.io app config.
  Mounts a persistent volume to `/data`. Health check + auto-start.
  Cliptown is single-process today; scale up VM size, not replicas.
- **`docs/DEPLOY.md`** (new) — quickstart for docker-compose, full
  Fly.io walkthrough (launch / volume / secrets / deploy / rotate /
  rollback), sketches for AWS / GCP / K8s / bare VPS, secrets pattern
  doc (`CLIPTOWN_OPERATOR_TOKEN`, `CLIPTOWN_AGENT_SECRET_*`, provider
  keys).
- **`README.md`** — adds a Deploy section pointing at DEPLOY.md.

Verified: `docker build` succeeds, `docker run` boots the world
server, `/health` returns `{"ok": true}` on first request.

### Known limitations carried forward

- `scripts/smoke-real-llm.sh` boots its own world; parameterizing it
  to target a remote deploy is a follow-up. DEPLOY.md documents the
  manual verification path in the meantime.

## M12 — P2.2 skills system (Phase 2 MVP, 2026-05-13)

Per-startup reusable markdown skills attached many-to-many to agents.
At `--real` adapter spawn the worker fetches the agent's attached
skills and writes each as `<workdir>/skills/<name>.md` (alongside
CLAUDE.md and the workspaces symlink from P2.3). CLAUDE.md gains an
"Available skills" section listing each skill's name and relative
path.

- **Schema:** `skills` (workspace-scoped, `UNIQUE(startup_id, name)`)
  + `agent_skills` (M:N attachment). Migration `0002_skills.sql`.
- **DAO:** `crates/world/src/skills.rs` with 9 inline unit tests
  (upsert/list/attach/detach/delete/for_agent + bad-name +
  oversize-content + cross-startup). Names constrained to
  `[A-Za-z0-9_-]{1,64}` (filesystem-safe); content capped at 64 KB.
- **MCP tools (5 new, catalog 16 → 21):** `skill_upsert`,
  `skill_list`, `skill_attach`, `skill_detach`, `skill_delete`.
  All enforce cross-startup checks.
- **HTTP API:** `GET /api/agents/:agent_id/skills` returns
  `{skills: [{name, content_md}]}` for the worker. Bearer auth via
  `<agent_id>:<secret>` reuses `crate::auth::validate_agent_secret`
  (env-var-backed; default `dev-secret`). 403 on path-vs-bearer
  agent mismatch; 401 on bad secret.
- **Worker:** new `skills_client.ts::fetchSkillsForAgent` +
  `prepareWorkdir` extension write skills into the execenv. Warn-
  and-continue on fetch failure (an agent with no skills proceeds
  with an empty list).
- **Smoke:** seeds `smoke-skill-deploy` + attaches it (§5.5);
  post-spawn asserts the file + CLAUDE.md reference (§7.6).

### Known limitations carried forward

- Skill content authoring UI (create/edit/delete) still routes through
  MCP `skill_upsert` / `skill_delete`. SkillsPanel covers read + attach
  + detach. Inline content editor is a future PR.
- No global (non-workspace) skills.
- No file attachments beyond the markdown content_md body.
- No versioning / history (upsert is mutable; latest wins).

## M12 — skills broadcasts + read+attach/detach UI (2026-05-13)

P2.2 follow-up that closes two of five known-limitations from the
initial Phase 2 MVP entry.

- **`ConsoleOutbound::SkillChanged`** broadcast on every skill
  mutation (upsert / delete / attach / detach). All 5 MCP skill_*
  handlers + the 2 new console-side arms emit. `kind="upsert"`
  carries the full listing row so the frontend reducer applies in
  place without a follow-up fetch.
- **`ConsoleOutbound::SkillsSnapshot`** delivered right after
  `WorldViewSnapshot` at console connect. Payload is `{sid: [{id,
  name, len, updated_at, attachments}]}` for every startup with
  skills.
- **`ConsoleInbound::SkillAttach` / `SkillDetach`**: operator-side
  attach/detach commands (no agent caller required). Skill authoring
  (create/edit/delete) still flows through MCP `skill_upsert` /
  `skill_delete` for now — heavier editor UX deferred.
- **`SkillsPanel.tsx`** in the operator console: lists skills for
  the currently-possessed startup, attached-agent chips (click to
  detach), "Attach to…" dropdown of unattached agents.

Known limitations retired by this section: "no frontend skill
management UI" partially (read + attach/detach is in; create/edit/
delete still via MCP) and "no `skill_changed` broadcasts" fully.

## M12 — P2.3 per-task execenv directories (2026-05-13)

Worker now creates a per-task execenv directory at
`<workspaces_root>/workspaces/<sid>/<tid>/workdir/` before each
`--real` adapter spawn, and uses that workdir as the adapter's cwd.

- **`packages/worker/src/execenv.ts`** (new) — `prepareWorkdir({
  workspacesRoot, startupId, taskId, agentId })` creates the workdir
  hierarchy, an absolute symlink `workdir/workspaces` →
  `<workspaces_root>/workspaces`, and an injected `CLAUDE.md` with
  agent_id / task_id / startup_id + the canonical artifact path
  contract. Idempotent.
- **`--task-id`** is now a required worker arg. The smoke script
  passes the existing `TASK_ID` value through.
- **No protocol / world / adapter changes.** The symlink lets the
  agent's existing relative path `workspaces/<sid>/artifacts/<tid>.md`
  resolve to the canonical location without touching the prompt or
  the world's `task_done` validator.
- **Reserves space for P2.2 (skills).** Sibling dirs under
  `<sid>/<tid>/` (e.g., `skills/`, `cache/`) are layout-ready; this
  PR doesn't ship any skill machinery.

### Known limitations carried forward

- No GC daemon for old execenv directories. Local smoke cleans up via
  tmpdir; long-running deployments will need a sweeper (separate
  follow-up).

## M12 — P2.1 daemon health buckets (2026-05-12)

Replaces cliptown's binary worker-liveness signal (WS connected vs
closed) with a 4-state Health enum so the operator console doesn't
confuse a 5-minute network blip with a hard crash.

- **`crates/world/src/health.rs`** (new pure module) — `Health` enum
  + `derive(now_ts, last_seen, connected, is_operator) -> Health`.
  Thresholds: `RecentlyLost` ≤ 5 min, `Offline` ≤ 6 d,
  `AboutToGc` ≤ 7 d (last 24 h before GC), beyond 7 d back to
  `Offline`. Operator avatars and clock skew both forced Online.
- **`AvatarView`** carries `last_seen_at: Option<i64>` (updated on
  `RegisterWorker` / `HandleWorkerMsg`, preserved through
  `UnregisterWorker`) and `health: Health` (refreshed every
  `Cmd::Tick` before the view broadcast). `RegisterWorker` /
  `UnregisterWorker` also derive health + broadcast immediately so
  the operator console reflects state changes without waiting for
  the next tick.
- **Frontend `AvatarVM`** mirrors the shape; `PixiStage.tsx` sets
  `sprite.alpha` from `ALPHA_BY_HEALTH` (`online: 1.0`,
  `recently_lost: 0.7`, `offline: 0.4`, `about_to_gc: 0.3`).
- **Tests:** 8 inline unit tests for `health::derive` + 3 integration
  tests booting `loop_::spawn` (register sets last_seen + Online;
  HandleWorkerMsg refreshes; Unregister preserves + RecentlyLost).

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

- Adapter budget tracking + hook flow: closed under M11 (PR #36).
- Frontend FCP bench: closed under M10.1 follow-up (PR #35) —
  `pnpm -F @cliptown/frontend bench:fcp` runs against a production
  `vite preview` build, no longer `.skip`'d.
- Criterion benches: closed under M11 real bench harness (this section).
- `bench.yml` CI gate runs as `continue-on-error: true` while baselines
  stabilize vs developer hardware. Flip to a hard gate after a handful
  of CI samples.

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
