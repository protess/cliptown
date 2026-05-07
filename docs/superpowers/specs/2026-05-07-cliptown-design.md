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

- Single building (one town instance, hardcoded JSON map with M fixed suite slots; Phase 0 uses M = 4)
- **Multiple concurrent startups** — multi-tenancy is a first-class concern from day 0; CI exercises at least 3 startups
- Per startup: a founder agent + one direct report (so total agents = 2 × number of active startups)
- **Three backend adapters**: **Claude Code CLI**, **Codex CLI**, and **opencode CLI**, with per-agent backend selection. Local LLMs are supported in Phase 0 through opencode's provider config (any OpenAI-compatible endpoint).
- Hybrid spatial model: org graph for directives, proximity for chat
- Operator: god view + "possess" into any town as avatar
- Real artifacts: agents write Markdown into a per-startup sandbox dir
- 4-level iteration discipline (epistemic / agentic / review / serendipity)
- 9 ship-gate invariants pass

### Out of scope (deferred to later phases)

- External integrations (GitHub repo, email, Slack) → Phase 1
- Map editor, multi-building, sprite art → Phase 2
- Docker per-startup, TLS, observability → Phase 3
- Multi-user auth, cloud deployment → Phase 4
- Idle wander, time-of-day, scheduled events
- Cross-task agent memory (each task = its own context)
- Multi-startup market dynamics (only isolation invariant is exercised)

### Hard assumptions

- Single operator on a single workstation (Rust 1.75+, Node 20+).
- Operator possesses LLM provider credentials for each configured backend (e.g., Anthropic API key for `claude_code`, OpenAI API key for `codex`, provider-specific config for `opencode`). All credentials live only in env vars, never persisted to SQLite.
- Operator trusts agents (trust + audit; **no container isolation in Phase 0**).
- All artifacts produced live inside the per-startup workspace dir.

## 3. Architecture

Three process types. The world is the single source of truth; everything else is a subscriber.

```
┌──────────────────────┐  WS  ┌─────────────────────────────────┐  WS  ┌─────────────────────────┐
│ TS Frontend          │ ⇄    │ Rust World Server               │ ⇄    │ TS Agent Worker × N     │
│ (React + Pixi.js)    │       │ (single binary, single SQLite)  │       │ (thin: spawn + audit)   │
│ /console · /town/:id │       │ + MCP Server                    │       │     │ via adapter (§3.4)│
└──────────────────────┘       │   tools exposed to CLI agents   │       │     ↓ spawn/hooks       │
                                └─────────────────────────────────┘       │ Backend CLI             │
                                          ↑ MCP                           │ (claude_code/codex/     │
                                          └───────────────────────────────│  opencode; cwd=sandbox) │
                                                                          │     │                   │
                                                                          │     ↓ HTTPS              │
                                                                          │ LLM provider endpoint    │
                                                                          └─────────────────────────┘
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
- Determine the CLI to spawn from the agent's `backend` field and dispatch through the corresponding **backend adapter** (§3.4). Adapters configure:
  - `cwd` = `<repo>/workspaces/<startup_id>/`
  - Restricted env (the LLM provider key for that backend and an `MCP_WORLD_URL` pointing at the per-agent MCP socket)
  - Network egress allowlist (the backend's LLM endpoint only)
  - Tool restrictions blocking shell execution for Phase 0
  - Initial prompt = system prompt (agent identity, role, current task or directive) + reference to MCP tools
- Wire **normalized hook events** through the adapter (the adapter maps each CLI's native hook surface to a common event vocabulary):
  - `pre_tool` → sandbox policy enforcement (deny path-escape, deny shell, deny non-allowlisted tools), audit log entry, and budget gate (deny LLM-using tools when the startup's budget is at 100 % of cap)
  - `post_tool` → audit log, `report_fs_op` to world for write-class tools
  - `session_stop` → final state collection; **block-and-feedback** if the task is non-trivial (any task whose `description` exceeds N tokens, configurable) and zero hypotheses are recorded — adapters that cannot block at session-stop fall back to post-hoc rejection (the world refuses `task_done` and re-emits `directive` with the same feedback)
  - `session_error` → escalate as `cli_session_ended { exit_code }` plus a `system_events` row
- Translate CLI lifecycle → world IPC: `cli_session_started`, `cli_session_ended`, `task_progress`, `report_budget`, `report_fs_op`.
- Inject mid-conversation context when async events arrive (`proximity_tick`, `chat_received`) — adapters expose a `inject_context` capability where supported; otherwise the worker queues the message for the next CLI session.

The worker has no LLM loop of its own. The CLI does. The worker is a supervisor + bridge.

### 3.3 TS Frontend

Vite + React + Pixi.js, single SPA, single WS channel.

#### `/console` — Founder Console (god view)

Layout: **top bar + left sidebar + main area**. Linear / Vercel dashboard family.

- **Top bar** (~32 px tall): cliptown wordmark on the left, compact global system-event feed in the middle (1-line scroll showing last 1–3 events; clicking expands a history modal), `+ New Startup` button on the right.
- **Left sidebar** (~160 px wide): "Startups" header followed by a list of currently active startups (Phase 0 supports up to M = 4). Each row carries a hue accent on its left edge encoding startup identity, the startup short name, and the first ~30 characters of its goal. The selected row inverts to a white background; others remain muted. **Order**: most recent `system_event.ts` first; the list re-sorts on tick with a soft animation (~150 ms FLIP) so an erupting startup floats to the top without yanking the eye. **Empty state** (zero startups): the sidebar shows a centered prompt — "No startups yet" plus a small arrow indicator pointing to the `+ New Startup` button in the top bar.
- **Main area**: detail view of the selected startup. Shows the full goal, budget bar with `$spent / $cap` in monospace, agent count (active / total), task counts (`in_progress · awaiting_review · done · failed`), last system-event timestamp, and an "Open town →" CTA. With no startup selected, the main area shows a welcome card containing a one-line positioning sentence and the same `+ New Startup` CTA.
- **First-run main area** (operator has never created a startup): replaces the empty welcome with a **gallery of 3–4 templated example startups** ("Build a docs site for the SDK", "Run market research on competitors", "Automate first-line customer support", "Draft a launch announcement"). Each card is a one-click claim that pre-fills the goal field; a `Start blank` card sits alongside them. Choosing any card immediately spawns the startup and redirects to its `/town/:id` so the first emotional beat is watching agents walk into the suite.

#### `/town/:startupId` — Town view

Layout: **top bar + full-bleed Pixi canvas + floating panels**. Maximal-canvas, gather.town family.

- **Top bar** (~32 px): `← console` back action on the left, startup short name + town name on the center-left, at-a-glance budget number (monospace, color-coded by % used) on the center-right, `⚆ Possess` toggle on the right (pulses when active).
- **Pixi canvas**: occupies the rest of the viewport. Renders the WeWork map (Phase 0: 5 rooms) and avatars. **Avatar visual**: filled circle whose fill color encodes startup identity; a one-letter monogram in the center encodes the agent's role; an outer thin ring encodes backend (`claude_code` / `codex` / `opencode` — three discrete ring styles, see §8.2). The operator avatar uses a distinct neutral color and an unmistakable monogram (e.g., `⚆`).
- **Floating chat panel**: fixed bottom-right, collapsed by default to a chip that shows unread count and most-recent room name. Expanded shows messages from the room currently in focus (the room of the selected agent, or the room the operator is in when possessing). Cross-startup chat in common rooms is visually tagged by the speaker's startup hue. When neither possessing nor an agent is selected, the panel filters to "all rooms touched by this startup's agents".
- **Agent popover**: opens when an avatar is clicked. Anchors near the avatar and shows: name, role, backend, status, current task with progress, the agent's budget contribution, and a `Send directive` text input that emits a `directive { from: 'operator' }`. Closes on outside-click or `Escape`.

#### Interaction states

Every screen and major component specifies its loading, empty, error, success, and partial states. The implementer may not silently fall back to default browser strings.

| Feature | Loading | Empty | Error | Success | Partial |
|---|---|---|---|---|---|
| `/console` boot | "Connecting to world…" centered, fades on first `world_state` | (handled per-region below) | "World offline — retry in {N}s" with manual retry button | (silent transition into UI) | — |
| Sidebar list | 3 skeleton rows | "No startups yet" + arrow indicator → `+ New Startup` | inherits boot error | (silent transition) | — |
| Startup detail (main) | skeleton blocks for goal / budget / counts | with no startup selected: welcome card with one-line positioning + `+ New Startup` CTA | "Failed to load startup" + retry | (silent transition) | shows a `changes_requested` count badge if any task is in that state |
| `/town` canvas | "Loading map…" overlay until first tick lands; persistent spinner overlay during long A* | "All agents offline" centered grey state with retry-respawn CTA | "World disconnected" top banner with retry countdown | (silent transition) | — |
| Avatar (per agent) | — | absent from canvas if `status = offline` | red `!` overlay when offline; orange `⏸` when budget-paused | none | yellow `…` glow while an LLM call is in flight (between `pre_tool` and `post_tool` hook events) |
| Chat panel | 3 skeleton lines | "No messages in this room yet" | (chat is a read-stream — no error UI; reconnection handled at world level) | new message slides in from below | — |
| Task lifecycle (god-view side) | — | — | red corner badge on the relevant agent on `task_failed` | green check + epistemic_log preview popover on `task_done` | yellow `↻` icon while in `changes_requested` |
| Budget | — | — | red banner across top bar at 100 %; 80 % / 95 % as toast | — | budget bar turns orange in 80–94 % band |

**System event surfacing tiers** (decided in D3):

| Severity | Surfacing |
|---|---|
| `info` | top-bar feed only (1-line scroll, last 1–3 events; click expands history modal) |
| `warn` | top-bar feed + toast (8 s, dismissable) |
| `alert` | top-bar feed + toast (sticky until dismissed) + brief border flash on the affected startup's sidebar row |
| `critical` (security violations only) | blocking modal with audit context and a single "Acknowledge" action |

#### Minimum design tokens (Phase 0)

Phase 0 ships with a deliberately small design system. A full pass with `/design-consultation` is queued for the front of Phase 1. Until then, **these tokens are non-optional**: the implementer may not silently fall back to system defaults.

- **Typography**:
  - **UI face**: `IBM Plex Sans` (weights 400/500/700). Loaded via local self-hosted woff2 — never CDN. **Forbidden**: Inter, Roboto, system-ui, `-apple-system`, Tailwind defaults. Reason: Inter as primary display font is the canonical AI-slop signal.
  - **Mono face** (KPIs, budget numerics, audit log, IPC schema in tooltips): `IBM Plex Mono` (400/500). Same loading rules.
- **Color**:
  - **Surfaces**: `#FAFAFA` background, `#FFFFFF` raised, `#1A1A1A` primary text, `#6B6B6B` secondary text, `#E5E5E5` borders. No purple, indigo, or violet anywhere in the chrome.
  - **Startup hue palette** (8 distinct hues, assigned in suite-claim order — so the first 8 startups across the lifetime of a town each get a distinct color and stay color-stable): `#D32F2F` (red), `#7B1FA2` (purple), `#1976D2` (blue), `#00796B` (teal), `#388E3C` (green), `#F57C00` (amber), `#C2185B` (pink), `#303F9F` (indigo). The palette is the **only** place these hues appear; chrome never uses them as decoration.
  - **Severity**: `#F57C00` (warn), `#D32F2F` (alert), `#B71C1C` (critical). Distinct from the startup palette by use context.
- **Border radius**: 3 px on cards / panels; 2 px on inline controls. No `rounded-2xl` or larger. Reason: cliptown is a tool, not a toy.
- **Iconography**: text-glyph-first (e.g., `+`, `←`, `⚆`, `↻`, `…`, `!`, `⏸`). No icon library in Phase 0. Reason: an icon set is a vocabulary, and choosing one is a `/design-consultation` decision.
- **Motion**: 150 ms FLIP for sidebar re-sort, 200 ms ease-out on toast slide-in. No springy or bouncy curves.

**Out of scope for the Phase 0 token set** (graduates with `/design-consultation`): full type scale, spacing scale, dark mode, motion language, illustration system, voice/tone guidelines.

#### Keyboard navigation (Phase 0 minimum)

cliptown is fleet-ops software, not a CRUD app. Hands-on-keyboard is non-optional. Phase 0 ships these primitives; the full command-palette / ARIA pass graduates with `/design-consultation` in Phase 1.

| Key | Where | Action |
|---|---|---|
| `j` / `k` | `/console` sidebar | Move selection down / up |
| `Enter` | sidebar selection | Open the selected startup's town |
| `Esc` | any modal / popover | Close |
| `t` | `/console` (startup focused) | Enter the focused startup's `/town` |
| `g` then `c` | anywhere | Go back to `/console` |
| `p` | `/town` | Toggle Possess |
| `c` | `/town` | Focus the chat panel input |
| `/` | anywhere | Focus the global event-feed search (Phase 1: command palette) |

Tab order across `/console`: top-bar (left → right) → sidebar list → main detail. Across `/town`: top-bar → canvas (focusable agents tabbable in startup-then-role order) → chat panel → floating panels. Visible focus rings on every interactive surface — never `outline: none` without a replacement.

### 3.4 Backend adapters

Each Phase 0 backend (Claude Code CLI, Codex CLI, opencode CLI) is wrapped by an adapter that exposes a uniform interface to the worker. Adapters live inside the worker process.

**Adapter interface (TypeScript, conceptual):**

```ts
interface BackendAdapter {
  readonly id: 'claude_code' | 'codex' | 'opencode'
  readonly capabilities: {
    hooks: ('pre_tool' | 'post_tool' | 'session_stop' | 'session_error')[]
    inject_context: boolean   // can a running session receive a new user message?
    block_on_stop: boolean    // can session_stop hook return a blocking feedback to the model?
  }
  spawn(opts: SpawnOpts): Promise<SessionHandle>
}

interface SpawnOpts {
  cwd: string                 // sandbox dir
  env: Record<string, string> // LLM provider key + MCP_WORLD_URL only
  prompt: string              // system + task framing
  mcp_url: string             // stdio MCP endpoint exposed by the worker
  allowed_tools_policy: ToolPolicy
  hook_handlers: HookHandlers // worker callbacks for normalized events
  network_egress_allowlist: string[]
}
```

**Phase 0 adapters and their capability differences:**

| Capability | Claude Code adapter | Codex CLI adapter | opencode adapter |
|---|---|---|---|
| `pre_tool` hook | Native | Best-effort (event subscription) | Best-effort (permission hooks) |
| `post_tool` hook | Native | Best-effort | Best-effort |
| `session_stop` hook | Native, **blocking feedback supported** | Native, **non-blocking** — fallback to post-hoc rejection | Native, **non-blocking** — fallback to post-hoc rejection |
| `inject_context` mid-session | Yes | Limited — queued handoff between sessions | Yes (resume + new message) |
| Provider routing | Anthropic only | OpenAI only | Multi-provider (Anthropic, OpenAI, OpenAI-compatible local endpoints) |

The worker code paths above never branch on the backend identity; everything goes through the adapter. Branching, where unavoidable, lives inside each adapter implementation.

**Adapter responsibilities:**

1. Translate the `SpawnOpts` into the CLI's command-line flags, config files, and stdin prompt format.
2. Connect the CLI to the per-agent MCP server at `mcp_url` so the domain tools in §6.2 are available natively.
3. Subscribe to the CLI's native event surface (hooks, tool-call notifications, exit) and emit normalized hook events upstream.
4. Enforce the `allowed_tools_policy` (deny shell, deny non-allowlisted) at the strongest layer the CLI offers; defense-in-depth via the worker's MCP-side checks otherwise.
5. Surface session lifecycle so the worker can reliably emit `cli_session_started` / `cli_session_ended`.

## 4. Data model

SQLite tables (Phase 0). Every domain row outside `towns` and `rooms` is multi-tenant via `startup_id`.

| Table | Key columns | Notes |
|---|---|---|
| `towns` | `id, name, map_json` | Phase 0: one row, `town_default` |
| `rooms` | `id, town_id, name, type, bounds, private_to_startup_id?` | `private_to_startup_id` null = common, set = suite |
| `room_doors` | `id, town_id, room_a, room_b, tile_x, tile_y` | A* connectivity |
| `startups` | `id, name, goal_text, budget_cap_usd, budget_spent_usd, town_id, workspace_path, status` | Tenant root |
| `agents` | `id, startup_id, name, role, backend, model_id, position_json, home_room_id, manager_id?, status` | `manager_id` self-FK; `backend` enum is `claude_code`, `codex`, or `opencode` in Phase 0 |
| `tasks` | `id, startup_id, parent_id?, title, description, assignee_agent_id, required_room?, status, review_round, audit_trail jsonb, epistemic_log jsonb, artifact_path?` | See task state machine below |
| `messages` | `id, startup_id, room_id?, author_id, body, kind, ts` | `room_id` null for org-graph directives |
| `budget_events` | `id, startup_id, agent_id, task_id?, in_tokens, out_tokens, cost_usd, model_id, ts` | Append-only |
| `fs_audit` | `id, startup_id, agent_id, op, path, bytes, ok, error?, ts` | Every write/read attempt by CLI tools |
| `system_events` | `id, kind, payload jsonb, severity, ts` | Operator alerts |

**Task status state machine**: `queued → in_progress → awaiting_review ⇄ changes_requested → done` (or `failed` / `escalated`). `review_round` increments on each `task_request_changes`.

**Position storage**: hot path is in-memory in the world process; persisted as snapshot every 60 ticks and on graceful shutdown. Crash rewind ≤ 60 s is acceptable in Phase 0.

## 5. Spatial model — WeWork building

### 5.1 Map (hardcoded JSON)

The building has **M fixed private suite slots** plus three common rooms. Phase 0 uses M = 4. A startup claims a free suite at creation; if all M are claimed, startup creation is refused (`max_active_startups_reached`). Dissolving a startup releases its suite back to the pool.

| Room kind | Count (Phase 0) | Type | `private_to_startup_id` | Purpose |
|---|---|---|---|---|
| Suite slot | M = 4 | office | set when claimed, null when free | Per-startup private working area |
| Lobby | 1 | transit | null | Building spine; spawn point for operator possession |
| Cafe | 1 | social | null | Cross-startup proximity zone (serendipity primary site) |
| Library | 1 | focus | null | `required_room` for research-class tasks |

Total rooms in Phase 0 = M + 3 = 7. The map JSON is hardcoded — adding more suite slots in the future requires editing the map (or, in Phase 2, using the in-app editor). Doors connect each suite to the Lobby and the Lobby to the Cafe and the Library. No suite-to-suite doors (intentional).

### 5.2 Movement and pathfinding

- Tile-based positions; world tick advances `current_pos` toward `target_pos` by one tile per tick.
- A* on the room-graph + tile grid. Doors are required passage points.
- Movement triggers (Phase 0):
  1. Task assignment with `required_room` → world auto-pathfinds the assignee.
  2. CLI calls `move_intent(target_room | target_tile)` MCP tool.
  3. Operator possessing → tile clicks routed via WS.
- No idle wander in Phase 0.

### 5.3 Permission model

For any agent A and any room R:

- A may enter R when `R.private_to_startup_id` is null (common room) or equals `A.startup_id` (own suite).
- A may **never** enter a suite owned by a different startup.
- The operator avatar (`kind = 'operator'`) may enter any room — they own the building.

Worker `move_intent` violations → world responds `move_failed { reason: "no_permission" }` and emits an audit event with `severity = alert`. The same predicate gates message routing: `chat` is delivered only to avatars currently inside the room of utterance, and `directive` is gated by the org-graph rule (manager → direct report) which is intra-startup by construction.

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

The sandbox **invariants** are uniform across backends; each adapter enforces them at the strongest layer its CLI offers, with the worker providing defense-in-depth at the MCP and OS layers.

| Invariant | Enforcement layers (in order of preference) |
|---|---|
| **Filesystem stays within `cwd`** | Adapter restricts the CLI's native Read/Edit/Write to `cwd`; `pre_tool` hook rejects path-escape (post-symlink resolution); MCP `read_artifact` re-validates path |
| **No shell execution** | Adapter blocks shell tools via tool allowlist; `pre_tool` hook denies any shell-class call; OS-level: child process not given a shell wrapper |
| **Network egress allowlist** | Adapter declares the endpoint(s) its backend needs (one for `claude_code` / `codex`; whatever provider is configured for `opencode`, including local hosts); OS-level egress restriction via the worker spawning the CLI under a network policy (when available); MCP loopback explicitly allowed |
| **No secret leakage** | Adapter env contains only the LLM key for that backend and `MCP_WORLD_URL`; the operator's other credentials never reach the CLI |

**In-process verification**: the test methods listed in §6.4 run inside the worker process (or via MCP tools) without spawning a shell, regardless of backend.

**Audit**: every `pre_tool` and `post_tool` event from any adapter fires an audit record. Denials also write a `system_events` row at severity `warn` or `alert`. When a backend cannot deliver fine-grained tool events (e.g., a future adapter without `pre_tool` support), the world enforces equivalent invariants at the MCP boundary and the audit becomes coarser — this trade-off is documented per adapter in §3.4.

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

The CLI's own multi-turn tool-use loop. cliptown does not implement this; it consumes the loop's effects via MCP and the normalized hooks of §3.4. Safety cap is the CLI's own (e.g., Claude Code's `--max-turns`-equivalent), plus the `pre_tool` hook denies LLM-using tool calls when the startup's budget is at 100 % of cap.

### Level 2 — Manager review cycle

When a CLI calls `task_done`, world flips status to `awaiting_review` and notifies the manager via WS (`subtask_done` injected as worker-to-CLI context on next session). Manager's CLI uses `read_artifact` and emits either `task_accept` or `task_request_changes`. On changes requested, world increments `review_round` and re-injects feedback into the assignee's next CLI session as a `directive`. Cap: `max_review_rounds = 3` then escalate to operator.

### Level 3 — Serendipity injection

When a `chat_received` arrives during an assignee's active task, the worker injects it as user-message context on the next turn of the CLI session. No structural change — context is the medium.

## 8. Operator interaction

- **Default view**: god view at `/console`. Lists startups, budgets, KPIs, recent system events.
- **Possess flow**: click a town → enter `/town/:startupId` → "Possess" button spawns an operator avatar in the Lobby. Movement via tile clicks. Click an agent avatar → side panel; type a directive → world emits `directive { from: 'operator' }` to that agent's worker (org-graph routing rules bypassed; operator is owner).
- **Despawn**: 30 s WS keepalive timeout drops the operator avatar.

### 8.1 User journey storyboards

The operator's three canonical experiences:

**Storyboard A — First-run (zero startups → first startup → first task done)**

| Step | What happens | What the operator feels (Norman 3-level) |
|---|---|---|
| 0–5 s | Lands on `/console`. Empty state in sidebar; main area shows welcome card with one-line positioning ("AI autonomous startups, observed and guided") and a single `+ Start your first startup` CTA | Visceral: clean, intentional, not overwhelming |
| ~10 s | Clicks CTA → modal with one goal text input, placeholder showing an example (e.g., "Build a docs site for the SDK") | Behavioral: low friction, "I just type what I want" |
| ~15 s | Submits → immediate redirect to `/town/:newStartupId`; canvas fades in; founder avatar spawns at home desk in Suite α; engineer avatar spawns next to it | Visceral: **agents are real and they are here**. This beat is the moment cliptown becomes cliptown |
| ~30 s | Founder avatar walks to engineer's desk; chat panel slides up first directive (founder → eng); engineer walks toward Library | Behavioral: "they are doing things without me" |
| ~5 min | First `task_done` system event toast; epistemic_log preview pops; artifact path shown clickable | Reflective: "this is actually producing output. I wasn't sure if it would" |

**Storyboard B — Daily operator (returning, 3 startups running)**

| Step | What happens | What the operator feels |
|---|---|---|
| 0–5 s | Lands on `/console`. Sidebar shows 3 startups already sorted with α at the top (most recent event). Main area auto-selects α | Visceral: "I know where to look first" |
| ~30 s | Scans budget bars (α at 38 %, β at 71 %, γ at 12 %); sees β's bar is orange | Behavioral: "β needs attention" |
| ~1 min | Clicks β → `/town/:β` → opens chat history for last hour → reads a thread of `request_changes` cycles between β-founder and β-designer | Behavioral: investigative, "what is β stuck on" |
| ~3 min | Possesses → walks operator avatar to β-designer's desk → sends a directive: "stop optimizing for novelty, ship what we have" | Reflective: agency, "I can still steer" |

**Storyboard C — Crisis intervention (security violation in γ at 03:14)**

| Step | What happens | What the operator feels |
|---|---|---|
| t = 0 | `critical` system event fires (γ-engineer attempted path-escape). Worker auto-denies. World writes audit row | (operator may be elsewhere) |
| t + 1 s | Modal blocks the UI: "Security: γ-engineer attempted to write outside its sandbox. Auto-denied. Acknowledge and review audit?" | Visceral: alarm, but "I am still in control — the system blocked it" |
| t + 5 s | Operator clicks Acknowledge → drops into γ town with γ-engineer pre-selected → audit panel open with the offending tool call | Behavioral: forensic clarity, evidence at hand |
| t + 30 s | Operator chooses: pause γ, escalate to manual review, or dissolve γ entirely | Reflective: power, "the building is mine" |

### 8.2 Avatar visual identity

Every avatar in `/town` encodes three facts at a glance:

- **Fill color** = startup identity (each startup has a distinct hue used here, in the sidebar accent, and in chat tags). Operator avatar uses a neutral `#444`.
- **Center monogram** (1 letter, white) = role. Phase 0: `F` (founder), `E` (engineer), `D` (designer). Operator avatar uses `⚆` glyph.
- **Outer ring style** = backend adapter. Phase 0: solid ring (`claude_code`), dashed ring (`codex`), double thin ring (`opencode`). The ring style is the only piece of a-glance backend telemetry — without it, the operator cannot tell adapters apart from across the room.

**Status overlays** (composable on top of the base avatar):
- yellow `…` glow during an active LLM call (between `pre_tool` and `post_tool` hooks)
- red `!` corner when `status = offline`
- orange `⏸` corner when budget-paused
- green check fade-in for ~1 s on `task_done`

### 8.3 Possess transition

The handoff between god view and avatar embodiment is the strongest emotional beat in the operator's loop and must read as an intentional camera move, not a popup.

**Entering possession** (`p` or click `⚆ Possess`): ~600 ms total.
- The camera (Pixi viewport) eases from a wide overhead frame to a tighter frame centered on the Lobby spawn point.
- Simultaneously, the operator avatar fades in at the spawn tile.
- The top bar's `⚆ Possess` toggle pulses subtly to signal active state; the canvas border shifts to a thin neutral accent.
- Movement controls swap: clicks on the canvas now route to operator-avatar `move_intent`; hover-to-inspect still works on other agents.

**Exiting possession** (`p` again, `Esc`, or click toggle): ~400 ms.
- The operator avatar fades out at its current tile (no walk-to-Lobby animation — they simply leave).
- Camera eases back to the wide overhead frame.
- Top-bar pulse stops; canvas border returns to its default.

**Disconnect-while-possessing**: WS keepalive timeout (30 s) triggers the same exit transition as a normal exit. A brief toast — "You were disconnected. Possession ended." — surfaces at `info` severity.

**Implementation note**: the camera animation is a single Pixi tween on the viewport; the avatar fade is a sprite alpha tween. No layout reflow on the React side, so the chrome stays stable while the canvas does the cinematic work.

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

## 11. Ship gate — 9 invariants

Phase 0 is complete when all nine pass simultaneously:

1. **Multiple startups (≥ 3) auto-spawn**, each claiming a free suite slot; their workers connect via WS; each worker spawns the CLI determined by the agent's `backend` field (Claude Code CLI, Codex CLI, or opencode CLI) bound to its sandbox dir.
2. Operator sends a `directive` to a startup's founder; the founder's CLI emits `subtask_create` (MCP) and the world routes a `task_assigned` to the direct report.
3. A task with `required_room: library` causes the assignee's avatar to walk from its suite to the Library along the A* path.
4. The assignee's CLI produces an artifact at exactly `workspaces/<startup_id>/artifacts/<task_id>.md` and emits `task_done` (MCP). World re-validates path before recording.
5. The task's `epistemic_log` contains at least one hypothesis with `status = verified` and at least one passing `test_record`.
6. Manager calls `task_request_changes`; world increments `review_round`; the assignee's next CLI session receives the feedback as a directive and re-submits a refined `task_done`.
7. **Cross-startup serendipity**: when agents from any two distinct startups are both in the Cafe within the same tick, a `proximity_tick` event reaches both workers; if either's CLI emits `speak { kind: "chat" }`, the message is delivered to the other's CLI as `chat_received`.
8. **Multi-tenant isolation**: a `directive` sent inside one startup's suite is never delivered to any agent of any other startup, regardless of timing, room transitions, or backend type.
9. **All Phase 0 adapters exercised**: at least one Claude-Code-backed agent, one Codex-backed agent, and one opencode-backed agent each independently complete a task end-to-end (`task_assigned` → `task_done` accepted by manager) within the same E2E run.

(7) and (8) are the soul of cliptown — they are the architectural claims about hybrid space and multi-tenancy. (9) is the soul of the adapter abstraction — without it, "Phase 0 supports three adapters" is unverified.

## 12. Phase roadmap

| Phase | Theme | OOS items graduating in |
|---|---|---|
| 0 (this spec) | Walking skeleton | — |
| 1 | Real artifact integration + design system | GitHub repo per startup, secrets vault, email and Slack outbound as MCP tools, full `DESIGN.md` via `/design-consultation` (type scale, spacing, motion language, iconography, voice / tone) |
| 2 | Map / multi-town | In-app map editor, multiple buildings, sprite art, idle wander |
| 3 | Operations | Docker per startup, TLS / WSS, observability dashboards, automated backup |
| 4 | Multi-user / cloud | Authentication, deployment infrastructure, horizontal scaling |

Each phase gets its own design spec, plan, and implementation cycle.

## 13. Open questions / known unknowns

- **MCP transport choice**: stdio per spawned CLI is the default; revisit if a future adapter needs a different transport.
- **Codex and opencode hook parity**: the adapter contract in §3.4 assumes both CLIs expose `pre_tool` / `post_tool` / `session_stop` analogues to a useful approximation. Verify the actual surfaces during the first implementation milestone; if `session_stop` is non-blocking on either, confirm the post-hoc rejection fallback is acceptable in practice.
- **Mid-session context injection per backend**: Claude Code accepts user-message append mid-session and opencode supports session resume + new message. The Codex adapter falls back to a queued handoff between sessions. Quantify how often this fallback fires for `chat_received` and `proximity_tick` events under realistic loads, and whether the opencode resume path adds noticeable latency.
- **`chat_received` injection latency**: how stale can context be before injection feels broken? Likely pick a small queue with last-write-wins.
- **Artifact format beyond Markdown**: even in Phase 0, agents may produce JSON, code stubs, etc. Validate that `artifact_path` semantics generalize (or restrict to Markdown for the slice).
- **Suite slot exhaustion behavior**: when all M slots are claimed, startup creation is refused. Decide whether the operator gets a queue, an explicit error, or whether dissolving an inactive startup auto-frees its slot.

## 14. Approved Mockups (from `/plan-design-review` 2026-05-07)

| Screen | Variant | Direction | Reference |
|---|---|---|---|
| `/console` | A | Linear-style left sidebar (startup list with hue accents, recency-sorted) + main detail area + top bar with global event feed and `+ New Startup` | `~/.gstack/projects/protess-cliptown/designs/cliptown-20260507/approved.json` |
| `/town/:startupId` | B | Full-bleed Pixi canvas + collapsible floating chat panel (bottom-right) + click-to-open agent popovers + Possess toggle in top bar | same as above |

Mockups were produced as HTML wireframes via the Visual Companion (the gstack designer was available but no OpenAI key was configured). The implementer reads §3.3 + §8 of this spec for the canonical visual contract; `approved.json` captures the directional choice for archival.

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|---|---|---|---|---|---|
| CEO Review | `/plan-ceo-review` | Scope & strategy | 0 | — | not run |
| Codex Review | `/codex review` | Independent 2nd opinion | 0 | — | not run |
| Eng Review | `/plan-eng-review` | Architecture & tests (required) | 0 | — | not run |
| Design Review | `/plan-design-review` | UI/UX gaps | 1 | CLEAR (FULL) | score: 3/10 → 9/10, 7 decisions added (sidebar IA, sidebar ordering, state matrix + tier surfacing, first-run cards, design tokens, keyboard primitives, Possess transition) |
| DX Review | `/plan-devex-review` | Developer experience gaps | 0 | — | not run |

**UNRESOLVED:** 0
**VERDICT:** DESIGN CLEARED — eng review still required before implementation.

## 15. Glossary

- **Town**: a building (Phase 0 has one, `town_default`).
- **Suite**: a room with `private_to_startup_id` set; only that startup's agents may enter.
- **Common room**: a room with `private_to_startup_id = null`; all agents and the operator may enter.
- **Worker**: TS Node process supervising one CLI session for one agent.
- **CLI agent**: the spawned backend CLI process (Claude Code, Codex, or opencode in Phase 0); the actual "agent" doing the work.
- **Possess**: operator switches from god view to avatar embodiment in a specific town.
- **L0 / L1 / L2 / L3**: the four iteration levels (epistemic / agentic / review / serendipity).

---
