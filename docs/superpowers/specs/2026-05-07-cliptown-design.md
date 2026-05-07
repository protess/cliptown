# cliptown — Phase 0 Design Spec

- **Date**: 2026-05-07
- **Status**: Draft (awaiting user review)
- **Author**: brainstorm session (james + assistant)
- **Scope**: Phase 0 — Walking skeleton vertical slice

---

## 1. Vision (one line)

> A WeWork-style coworking building where multiple AI autonomous startups run real work simultaneously, with a single human operator who watches from a god view and can possess into any town as an avatar.

The product *cliptown* is inspired by paperclip (AI company orchestration) and Gather.town (spatial collaboration). Inspiration only — no code reuse.

## 2. Phase 0 scope (and what is **not** Phase 0)

### In scope

- Single building (one town instance, hardcoded JSON map)
- 2 startups running concurrently, each in a private suite
- 4 agents total (per startup: founder + one direct report)
- 1 backend adapter: **Claude Code CLI**
- Hybrid spatial model: org graph for directives, proximity for chat
- Operator: god view + "possess" into any town as avatar
- Real artifacts: agents write Markdown into a per-startup sandbox dir
- 4-level iteration discipline (epistemic / agentic / review / serendipity)
- 8 ship-gate invariants pass

### Out of scope (deferred to later phases)

- Additional adapters (opencode, Codex CLI, local-LLM-via-opencode) → Phase 1
- External integrations (GitHub repo, email, Slack) → Phase 2
- Map editor, multi-building, sprite art → Phase 3
- Docker per-startup, TLS, observability → Phase 4
- Multi-user auth, cloud deployment → Phase 5
- Idle wander, time-of-day, scheduled events
- Cross-task agent memory (each task = its own context)
- Multi-startup market dynamics (only isolation invariant is exercised)

### Hard assumptions

- Single operator on a single workstation (Rust 1.75+, Node 20+).
- Operator possesses an Anthropic API key (held only in env var, never persisted).
- Operator trusts agents (trust + audit; **no container isolation in Phase 0**).
- All artifacts produced live inside the per-startup workspace dir.

## 3. Architecture

Three process types. The world is the single source of truth; everything else is a subscriber.

```
┌──────────────────────┐  WS  ┌─────────────────────────────────┐  WS  ┌─────────────────────┐
│ TS Frontend          │ ⇄    │ Rust World Server               │ ⇄    │ TS Agent Worker × N │
│ (React + Pixi.js)    │       │ (single binary, single SQLite)  │       │ (thin: spawn + audit)│
│ /console · /town/:id │       │ + MCP Server                    │       │      │              │
└──────────────────────┘       │   tools exposed to CLI agents   │       │      ↓ spawn/hooks  │
                                └─────────────────────────────────┘       │ Claude Code CLI     │
                                          ↑ MCP                           │ (cwd = sandbox dir) │
                                          └───────────────────────────────│      │              │
                                                                          │      ↓ HTTPS         │
                                                                          │ Anthropic API        │
                                                                          └─────────────────────┘
```

### 3.1 Rust World Server

Single binary, single SQLite file. Owns the world's truth.

- **HTTP API**: startup CRUD, map fetch, operator controls. Used by Founder Console.
- **WebSocket hub**: bi-directional with frontend (presence·chat·move) and with workers (supervise·audit channel).
- **MCP server**: exposes domain tools (move, speak, task lifecycle, hypothesis/test, observe) to CLI agents inside workers. Stdio MCP per spawned CLI, scoped to one agent's identity.
- **World tick**: 1 Hz. Advances avatar positions, evaluates proximity events, processes task scheduler.
- **Supervisor**: per agent, ensures one worker (and therefore one CLI session) is healthy. Exponential backoff respawn.
- **Persistence writes**: only the world writes to SQLite.

### 3.2 TS Agent Worker (thin)

One Node process per agent. Spawned and killed by the world. Its job is **infrastructure, not intelligence**.

Responsibilities:
- WS connect to world; handle `hello`, `world_state`, `task_assigned`, `directive`, `proximity_tick`, `chat_received`, `pause`, `shutdown`.
- Spawn the chosen CLI (Claude Code CLI in Phase 0) as a child process with:
  - `cwd` = `<repo>/workspaces/<startup_id>/`
  - Restricted env (only `ANTHROPIC_API_KEY` and an `MCP_WORLD_URL` pointing at the per-agent MCP socket)
  - Network egress allowlist (Anthropic endpoint only)
  - `allowedTools` config blocking `Bash` for Phase 0
  - Initial prompt = system prompt (agent identity, role, current task or directive) + reference to MCP tools
- Wire **Claude Code hooks**:
  - `PreToolUse` → sandbox policy enforcement (deny path-escape, deny `Bash`, deny non-allowlisted tools), audit log entry, and budget gate (deny LLM-using tools when the startup's budget is at 100 % of cap)
  - `PostToolUse` → audit log, `report_fs_op` to world for write-class tools
  - `Stop` → final state collection; **block-and-feedback** if the task is non-trivial (any task whose `description` exceeds N tokens, configurable) and zero hypotheses are recorded
- Translate CLI lifecycle → world IPC: `task_progress`, `task_done`, `task_failed`, `report_budget`.
- Inject mid-conversation context when async events arrive (`proximity_tick`, `chat_received`) — Claude Code accepts user-message append mid-session.

The worker has no LLM loop of its own. The CLI does. The worker is a supervisor + bridge.

### 3.3 TS Frontend

Vite + React + Pixi.js, single SPA, single WS channel.

- `/console` — Founder Console (god view): list of startups, KPIs, create/configure, navigate into a town.
- `/town/:startupId` — Pixi canvas + chat panel + "Possess" toggle. When possessed, the operator avatar enters the town with `kind: 'operator'` and movement is driven by client clicks.

## 4. Data model

SQLite tables (Phase 0). Every domain row outside `towns` and `rooms` is multi-tenant via `startup_id`.

| Table | Key columns | Notes |
|---|---|---|
| `towns` | `id, name, map_json` | Phase 0: one row, `town_default` |
| `rooms` | `id, town_id, name, type, bounds, private_to_startup_id?` | `private_to_startup_id` null = common, set = suite |
| `room_doors` | `id, town_id, room_a, room_b, tile_x, tile_y` | A* connectivity |
| `startups` | `id, name, goal_text, budget_cap_usd, budget_spent_usd, town_id, workspace_path, status` | Tenant root |
| `agents` | `id, startup_id, name, role, backend, model_id, position_json, home_room_id, manager_id?, status` | `manager_id` self-FK; `backend` enum (`claude_code` only in Phase 0) |
| `tasks` | `id, startup_id, parent_id?, title, description, assignee_agent_id, required_room?, status, review_round, audit_trail jsonb, epistemic_log jsonb, artifact_path?` | See task state machine below |
| `messages` | `id, startup_id, room_id?, author_id, body, kind, ts` | `room_id` null for org-graph directives |
| `budget_events` | `id, startup_id, agent_id, task_id?, in_tokens, out_tokens, cost_usd, model_id, ts` | Append-only |
| `fs_audit` | `id, startup_id, agent_id, op, path, bytes, ok, error?, ts` | Every write/read attempt by CLI tools |
| `system_events` | `id, kind, payload jsonb, severity, ts` | Operator alerts |

**Task status state machine**: `queued → in_progress → awaiting_review ⇄ changes_requested → done` (or `failed` / `escalated`). `review_round` increments on each `task_request_changes`.

**Position storage**: hot path is in-memory in the world process; persisted as snapshot every 60 ticks and on graceful shutdown. Crash rewind ≤ 60 s is acceptable in Phase 0.

## 5. Spatial model — WeWork building

### 5.1 Map (hardcoded JSON)

Five rooms in `town_default` (40 × 24 tiles, 1 tile = 32 px):

| Room | Type | `private_to_startup_id` | Purpose |
|---|---|---|---|
| Suite A | office | startup_α | Startup α private suite |
| Suite B | office | startup_β | Startup β private suite |
| Lobby | transit | null | Building spine; spawn point for operator possession |
| Cafe | social | null | Cross-startup proximity zone (serendipity primary site) |
| Library | focus | null | `required_room` for research-class tasks |

Doors connect each suite to the Lobby and the Lobby to Cafe and Library. No suite-to-suite door (intentional).

### 5.2 Movement and pathfinding

- Tile-based positions; world tick advances `current_pos` toward `target_pos` by one tile per tick.
- A* on the room-graph + tile grid. Doors are required passage points.
- Movement triggers (Phase 0):
  1. Task assignment with `required_room` → world auto-pathfinds the assignee.
  2. CLI calls `move_intent(target_room | target_tile)` MCP tool.
  3. Operator possessing → tile clicks routed via WS.
- No idle wander in Phase 0.

### 5.3 Permission model

| Subject | Suite A | Suite B | Lobby / Cafe / Library |
|---|:-:|:-:|:-:|
| α agent | ✓ | ✗ | ✓ |
| β agent | ✗ | ✓ | ✓ |
| Operator (possessing) | ✓ | ✓ | ✓ |

Violations from a worker's `move_intent` → world responds `move_failed { reason: "no_permission" }` and emits an audit event.

### 5.4 Message channels (hybrid model)

| `kind` | Routing rule | Cross-startup visible? |
|---|---|---|
| `directive` | Org graph: `manager → report` (room-independent) | No (intra-startup only, enforced by world) |
| `chat` | Spatial: all avatars in the same `room_id` receive | Yes if room is common; no if room is a suite |
| `system` | Broadcast to all agents in the affected startup | No (scoped) |

This is the literal implementation of "fixed org lines = graph; serendipity = space."

## 6. Worker contract

### 6.1 WS channel — supervise + audit (between Worker and World)

**Inbound (World → Worker)**

| Type | Payload | Trigger |
|---|---|---|
| `world_state` | snapshot: position, current room, current task, peers, recent messages | On worker connect |
| `task_assigned` | `{ task_id, title, description, required_room?, parent_id? }` | Scheduler routes a queued task |
| `directive` | `{ from_agent_id, body, in_response_to_task? }` | Manager or operator |
| `proximity_tick` | `{ room_id, members: [{ agent_id, name, role, startup_id }] }` | Each tick where ≥ 2 avatars share a room |
| `chat_received` | `{ from_agent_id, body, room_id }` | Same-room utterance |
| `move_complete` | `{ room_id }` | Pathfinding finished |
| `budget_warning` | `{ remaining_usd, percent_used }` | 80 % crossed |
| `pause` / `shutdown` | — | Budget cap, dissolve |

**Outbound (Worker → World)**

| Type | Payload | Notes |
|---|---|---|
| `hello` | `{ agent_id, startup_id, secret }` | First message |
| `cli_session_started` | `{ task_id?, prompt_hash }` | When worker spawns CLI |
| `cli_session_ended` | `{ task_id?, exit_code, summary? }` | When CLI exits |
| `task_progress` | `{ task_id, note }` | Hook-derived (PostToolUse summary) |
| `report_budget` | `{ in_tokens, out_tokens, model_id, task_id? }` | After every CLI LLM turn (PostToolUse hook) |
| `report_fs_op` | `{ op, path, bytes, ok, error? }` | For every CLI file write |

Domain actions (move, speak, task lifecycle, hypothesis/test, verification) are **not** on this channel. They are MCP tools called by the CLI directly (next section). Task completion and failure are recorded via the `task_done` / `task_failed` MCP tools; the worker does not echo them on WS.

### 6.2 MCP tools — domain surface (called by CLI)

Each spawned CLI gets a stdio MCP connection to the world, scoped to that one agent's identity. Tools:

| Tool | Args | Permission |
|---|---|---|
| `move_intent` | `{ target_room? \| target_tile? }` | Any agent. Async; returns when arrived. |
| `speak` | `{ body, kind: "chat"\|"directive", to_agent_id? }` | `chat` = current room. `directive` = direct report only. |
| `task_done` | `{ task_id, artifact_path }` | Assignee only. World re-validates path. |
| `task_failed` | `{ task_id, reason }` | Assignee only. |
| `subtask_create` | `{ parent_id, title, description, assignee_agent_id, required_room? }` | Manager only. |
| `task_accept` | `{ task_id }` | Manager of that task. |
| `task_request_changes` | `{ task_id, feedback, in_response_to_round }` | Manager of that task. |
| `hypothesis_state` | `{ task_id, id, claim, rationale }` | Assignee. Appended to `epistemic_log`. |
| `test_record` | `{ task_id, hypothesis_id, id, method, params, expected, observed, outcome }` | Assignee. Appended to `epistemic_log`. |
| `hypothesis_resolve` | `{ task_id, id, status, note }` | Assignee. Appended to `epistemic_log`. |
| `verify` | `{ method, params }` → `{ observed }` | Any agent. Runs an in-process check (see §6.4). Returns observation; CLI then logs via `test_record`. |
| `ask_peer` | `{ body, to_agent_id?, timeout_ms }` → `{ response? }` | Any agent. Speaks (chat in current room or directive to direct report) and awaits a single reply within timeout. |
| `observe_world` | `{ query: "peers_in_room"\|"my_position"\|"budget_remaining" }` | Any agent. Read-only. |
| `read_artifact` | `{ path }` | Same-startup only. Used by managers during review. |

Permission violations return MCP errors and emit audit events.

### 6.3 Sandbox model

- **Filesystem**: CLI's native Read / Edit / Write tools operate inside `cwd`. Worker's PreToolUse hook rejects any path that resolves outside `cwd` after symlink resolution.
- **Shell**: `Bash` tool is **denied** in Phase 0 via `allowedTools` config.
- **Network**: process-level egress allowlist (Anthropic endpoint only). MCP loopback explicitly allowed.
- **In-process verification**: the test methods listed below are implemented inside the worker (or invoked via MCP tools) without spawning shell.
- **Audit**: every PreToolUse and PostToolUse fires an audit record. Any denial is also a `system_events` row at severity `warn` or `alert`.

### 6.4 L0 epistemic test methods (no shell)

These are the `method` values accepted by the `verify` MCP tool (and, for `ask_peer` / `observe_world`, separate top-level MCP tools). All implementations are in-process inside the worker — no shell exec.

| `method` | Implementation | Purpose |
|---|---|---|
| `read_assert` | readFile + pattern / count / JSON-path check | Validate artifact structure |
| `lint_markdown` | `markdown-it` AST traversal | Heading structure, link form |
| `lint_typescript` | TypeScript Compiler API in-process | Parse / type-check |
| `lint_json` / `lint_yaml` | Schema validator | Config files |

`ask_peer` and `observe_world` are first-class MCP tools (see §6.2) rather than `verify` methods because they involve world-state queries or peer interaction, not pure local computation.

## 7. Iteration model — four levels

### Level 0 — Hypothesis · Test · Observe

Inside any agent's task work. The CLI calls `hypothesis_state`, `test_record`, `hypothesis_resolve`. Outputs structured `epistemic_log` entries.

Enforcement:
- **Prompt template** in CLI startup includes the Level 0 contract.
- **Stop hook** blocks `task_done` if the task is non-trivial and zero hypotheses are recorded; returns structured feedback to the CLI to add hypotheses first.
- Safety cap: `max_hypotheses_per_task = 8`, `max_tests_per_hypothesis = 5`.

### Level 1 — Agentic loop

The CLI's own multi-turn tool-use loop. cliptown does not implement this; it consumes the loop's effects via MCP and hooks. Safety cap is the CLI's own (Claude Code's `--max-turns`-equivalent), plus PreToolUse hook denies tool calls that would exceed budget.

### Level 2 — Manager review cycle

When a CLI calls `task_done`, world flips status to `awaiting_review` and notifies the manager via WS (`subtask_done` injected as worker-to-CLI context on next session). Manager's CLI uses `read_artifact` and emits either `task_accept` or `task_request_changes`. On changes requested, world increments `review_round` and re-injects feedback into the assignee's next CLI session as a `directive`. Cap: `max_review_rounds = 3` then escalate to operator.

### Level 3 — Serendipity injection

When a `chat_received` arrives during an assignee's active task, the worker injects it as user-message context on the next turn of the CLI session. No structural change — context is the medium.

## 8. Operator interaction

- **Default view**: god view at `/console`. Lists startups, budgets, KPIs, recent system events.
- **Possess flow**: click a town → enter `/town/:startupId` → "Possess" button spawns an operator avatar in the Lobby. Movement via tile clicks. Click an agent avatar → side panel; type a directive → world emits `directive { from: 'operator' }` to that agent's worker (org-graph routing rules bypassed; operator is owner).
- **Despawn**: 30 s WS keepalive timeout drops the operator avatar.

## 9. Resilience

| Failure | Recovery | Automatic? |
|---|---|---|
| World crash | Process supervisor restart → SQLite rehydrate → workers and frontend reconnect | ✓ |
| Worker crash | World detects WS disconnect → backoff respawn (1 s, 5 s, 30 s) × 3 → new CLI session resumes from `epistemic_log` | ✓ |
| CLI exit (non-zero) | Worker reports `cli_session_ended { exit_code }` → world treats as `task_failed` if no `task_done` MCP call landed | ✓ |
| LLM API failure | CLI handles internal retry; persistent failure surfaces as CLI exit | ✓ |
| Tool call denied (PreToolUse) | CLI receives error in tool result; chooses retry, alternative, or `task_failed` | ✓ |
| Sandbox path-escape attempt | Denied + `system_events` severity `alert` → operator notified | Alert |
| Cross-startup permission violation | `move_failed` or MCP error + `system_events` severity `alert` | Alert |
| Budget 80 / 95 / 100 % | Warn / no-new-task / pause-all. Operator raises cap to resume | Semi |
| Pathfinding impossible | `move_failed { no_path }` → CLI replans | ✓ |
| Operator disconnect during possession | 30 s timeout despawns avatar | ✓ |

Design principle: **the world only dies when it dies; workers die freely**. All task truth is in SQLite; workers are stateless and reconstruct from `epistemic_log` plus the task description.

## 10. Testing strategy

### Pyramid

- **Unit (Rust)**: tick logic, A*, task state machine, budget math, permission predicates.
- **Unit (TS Worker)**: IPC parser/serializer, sandbox path resolver (path-escape fixture battery), prompt template builder, `epistemic_log` shape, hook handlers.
- **Property tests (Rust, proptest)**: multi-tenant isolation invariants — "no message of `kind = directive` crosses `startup_id` boundaries"; "no agent enters a suite owned by a different startup."
- **Integration (Rust)**: world + N fake WS workers + fake MCP clients; golden traces.
- **Contract (TS)**: worker against a fake world emitting fixed event sequences.
- **E2E (Playwright)**: drives the demo described in §11.

### LLM cost control in CI

- Default: deterministic LLM mock (no real API calls).
- Real-LLM E2E only on `E2E_LLM=real` opt-in jobs, with a per-run budget cap (e.g. $0.50). Exceeding the cap fails the run.

## 11. Ship gate — 8 invariants

Phase 0 is complete when all eight pass simultaneously:

1. Two startups auto-spawn; four workers connect via WS; each worker spawns a Claude Code CLI session bound to its sandbox dir.
2. Operator sends a `directive` to a startup's founder; the founder's CLI emits `subtask_create` (MCP) and the world routes a `task_assigned` to the direct report.
3. A task with `required_room: library` causes the assignee's avatar to walk from its suite to the Library along the A* path.
4. The assignee's CLI produces an artifact at exactly `workspaces/<startup_id>/artifacts/<task_id>.md` and emits `task_done` (MCP). World re-validates path before recording.
5. The task's `epistemic_log` contains at least one hypothesis with `status = verified` and at least one passing `test_record`.
6. Manager calls `task_request_changes`; world increments `review_round`; the assignee's next CLI session receives the feedback as a directive and re-submits a refined `task_done`.
7. **Cross-startup serendipity**: when an α agent and a β agent are both in the Cafe within the same tick, a `proximity_tick` event reaches both workers; if either's CLI emits `speak { kind: "chat" }`, the message is delivered to the other's CLI as `chat_received`.
8. **Multi-tenant isolation**: a `directive` sent inside Suite A is never delivered to any agent of startup β, regardless of timing or room transitions.

(7) and (8) are the soul of cliptown. If either fails, the slice is not done.

## 12. Phase roadmap

| Phase | Theme | OOS items graduating in |
|---|---|---|
| 0 (this spec) | Walking skeleton | — |
| 1 | Adapter expansion | opencode adapter (incl. local-OpenAI endpoint), Codex CLI adapter, per-agent backend selection |
| 2 | Real artifact integration | GitHub repo per startup, secrets vault, email and Slack outbound as MCP tools |
| 3 | Map / multi-town | In-app map editor, multiple buildings, sprite art, idle wander |
| 4 | Operations | Docker per startup, TLS / WSS, observability dashboards, automated backup |
| 5 | Multi-user / cloud | Authentication, deployment infrastructure, horizontal scaling |

Each phase gets its own design spec, plan, and implementation cycle.

## 13. Open questions / known unknowns

- **MCP transport choice**: stdio per spawned CLI is the default; revisit if multiple CLI brands need a different transport.
- **Hooks for non-Claude-Code adapters**: opencode and Codex CLI's hook surfaces differ. Phase 1 must define an adapter interface that abstracts hook capabilities.
- **Operator directive injection mid-CLI-session**: Claude Code accepts mid-session user messages; verify behavior with hooks and confirm interleaving works in practice.
- **`chat_received` injection latency**: how stale can context be before injection feels broken? Likely pick a small queue with last-write-wins.
- **Artifact format beyond Markdown**: even in Phase 0, agents may produce JSON, code stubs, etc. Validate that `artifact_path` semantics generalize (or restrict to Markdown for the slice).

## 14. Glossary

- **Town**: a building (Phase 0 has one, `town_default`).
- **Suite**: a room with `private_to_startup_id` set; only that startup's agents may enter.
- **Common room**: a room with `private_to_startup_id = null`; all agents and the operator may enter.
- **Worker**: TS Node process supervising one CLI session for one agent.
- **CLI agent**: the spawned Claude Code CLI process; the actual "agent" doing the work.
- **Possess**: operator switches from god view to avatar embodiment in a specific town.
- **L0 / L1 / L2 / L3**: the four iteration levels (epistemic / agentic / review / serendipity).

---
