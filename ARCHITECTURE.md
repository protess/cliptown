# Architecture

cliptown is one Rust world server + one frontend + N adapter-spawned
CLI children, all on a single host (today). This doc walks the layout,
data flow, and key invariants.

## Topology

```
                ┌──────────────────────────────────────┐
                │       Operator Console (browser)     │
                │  React + Pixi 2D canvas + sidebar    │
                └──────────────┬───────────────────────┘
                               │ WS  /ws/console
                               ▼
              ┌────────────────────────────────────┐
              │             cliptown-world         │
              │ ┌────────────────────────────────┐ │
              │ │  axum HTTP + WS surface        │ │
              │ │  /health  /api/*  /ws/*  /mcp  │ │
              │ └─────────┬──────────────────────┘ │
              │           │                        │
              │ ┌─────────▼──────────────────────┐ │
              │ │     mpsc-routed event loop     │ │
              │ │  Cmd::{Tick, Console, Worker,  │ │
              │ │  Register, Insert, ...}        │ │
              │ └─────────┬──────────────────────┘ │
              │           │                        │
              │ ┌─────────▼──────────────────────┐ │
              │ │     SQLite (WAL)               │ │
              │ │  startups / agents / tasks /   │ │
              │ │  messages / skills / ...       │ │
              │ └────────────────────────────────┘ │
              │                                    │
              │  ┌──────────────────────────────┐  │
              │  │   AgentSupervisor (spawns)   │  │
              │  └────────────┬─────────────────┘  │
              └───────────────┼────────────────────┘
                              │ child process
                              ▼
                  ┌────────────────────────┐
                  │     worker (Node)      │
                  │  WS to /ws/worker      │
                  │  spawns adapter CLI    │
                  └───────────┬────────────┘
                              │ child process
                              ▼
                  ┌─────────────────────────────┐
                  │   adapter: claude / codex / │
                  │   opencode CLI              │
                  │   MCP over HTTP → world/mcp │
                  └─────────────────────────────┘
```

Workers spawn from the world's `AgentSupervisor` (a sibling task on
the same host). Adapter CLIs run as children of the worker. MCP traffic
flows CLI → world's `/mcp` HTTP directly (the worker is a process
supervisor, not an MCP proxy).

## Components

### `crates/world/` — Rust world server

Single-process. One `tokio::main` host, one mpsc inbox for all state
mutations, SQLite (WAL) for persistence, axum for the HTTP + WS surface.

Key modules:

- `loop_.rs` — the mpsc event loop. `Cmd::Tick` runs the simulation
  step; `Cmd::HandleConsoleMsg` / `HandleWorkerMsg` dispatch operator
  + agent input; `Cmd::Register / Unregister` track WS-connected
  workers; `Cmd::Insert / Release` track startup lifecycle.
- `mcp_dispatch.rs` — 21 MCP tools. Includes `task_done`,
  `task_failed`, `subtask_create`, `task_accept`,
  `task_request_changes`, `accept_proposal`, `reject_proposal`,
  `hypothesis_state`, `test_record`, `hypothesis_resolve`,
  `read_artifact`, `observe_world`, `move_intent`, `speak`,
  `ask_peer`, `verify`, `skill_upsert`, `skill_list`, `skill_attach`,
  `skill_detach`, `skill_delete`. All require bearer
  `<agent_id>:<secret>` auth.
- `mcp_http.rs` — `/mcp` HTTP route + tools/list catalog.
- `http.rs` — axum router. `/health`, `/api/startups`,
  `/api/agents/:id/skills`, `/ws/console`, `/ws/worker`, `/mcp`.
- `cmd_console.rs` — operator-side dispatcher. Handles
  `OperatorPossess`, `OperatorMove`, `OperatorDirective`,
  `OperatorAcceptProposal`, `OperatorRejectProposal`,
  `OperatorForceAccept`, `OperatorForceFail`, `SkillAttach`,
  `SkillDetach`.
- `agent_supervisor.rs` — spawns + respawns worker processes.
  Backoff: 1s, 5s, 30s.
- `scheduler.rs` — per-tick task assignment.
- `move_sys.rs` + `path.rs` — A* pathfinding for avatar positions.
- `proximity.rs` — agent-to-agent proximity broadcasts.
- `budget.rs` — per-startup spend tracking, warn/95%/pause ladder.
- `seed.rs` — initial town layout (rooms, doors).
- `state.rs` — `WorldView`, `AvatarView` (in-memory shape).
- `skills.rs` — P2.2 skills DAO. `upsert/list/attach/detach/delete/
  for_agent/list_with_attachments/list_all_with_attachments`.
- `health.rs` — P2.1 worker health bucket derivation.
- `api_skills.rs` — `GET /api/agents/:id/skills` for workers.
- `api_startups.rs` — startup CRUD.
- `protocol/ws_messages.rs` — wire types (auto-exported to TS via
  ts-rs).

### `packages/worker/` — Node worker process

Connects to `/ws/worker` with bearer `<agent_id>:<secret>`, then in
`--real` mode:

1. Fetches the agent's attached skills via
   `GET /api/agents/<agent_id>/skills`.
2. Calls `prepareWorkdir({ workspacesRoot, startupId, taskId,
   agentId, skills })` to create the per-task execenv (P2.3).
3. Spawns the chosen adapter (claude-code / codex / opencode) with
   `cwd=<workdir>`.
4. Pipes hooks + stdio to the world via `console.log`.
5. Forwards `UsageReport` to the world for budget telemetry.

Key files:
- `src/main.ts` — entry point + arg parsing.
- `src/execenv.ts` — `prepareWorkdir` (P2.3).
- `src/skills_client.ts` — `fetchSkillsForAgent` (P2.2).
- `bin/worker` — production entrypoint (tsx wrapper).
- `bin/fixture-cli` — synthetic claude-code shim for contract tests.

### `packages/adapters/` — backend CLI adapters

One adapter per CLI vendor. Shared `BackendAdapter` contract in
`adapters/core/`:

```ts
interface BackendAdapter {
  spawn(opts: SpawnOpts): SpawnedAdapter;
}
interface SpawnOpts {
  bin?: string;       // for fixture override
  prompt: string;
  cwd: string;
  worldUrl: string;
  agentToken: string;
  onHook: (e: HookEvent) => void;
  onLog: (stream, line) => void;
}
```

Per-adapter specifics:

- `claude-code/` — `claude --print --json` + `--settings` for hook
  config. PreToolUse/PostToolUse/Stop hooks fire via per-spawn
  settings file. Bridge listens on a 127.0.0.1 random port.
- `codex/` — `codex exec --json`. In-process JSONL parser
  (`event_parser.ts`) converts `item.started` / `item.completed` to
  pre/post tool HookEvents. No HTTP bridge.
- `opencode/` — `opencode serve` headless server + `/event` SSE.
  `sse_client.ts` + `event_mapper.ts` produce true pre/post semantics
  from `pending → running → completed` state transitions.

All three honor `opts.cwd` and pass `--mcp-config` (or equivalent)
pointing at the world's `/mcp` HTTP endpoint with the agent's bearer.

### `packages/frontend/` — React + Pixi operator console

- `src/store.ts` — `WorldState` reducer. Hydrates from
  `world_view_snapshot` and `skills_snapshot` on connect, then
  applies deltas + targeted events (`skill_changed`, `chat`,
  `directive`, `system_event`).
- `src/console/` — sidebar panels (Kanban, ChatPanel, SkillsPanel,
  HistoryModal, TopBar).
- `src/town/PixiStage.tsx` — 2D avatar canvas.
- `src/ws.ts` — `ConsoleClient` WS wrapper.

### `packages/protocol/` — ts-rs auto-generated types

Built from `crates/world/src/protocol/ws_messages.rs` and other types
that derive `#[derive(TS)]`. Frontend imports these directly for
wire-format type safety.

## MCP tools

21 tools as of M12. Catalog returned by `POST /mcp` with method
`tools/list`. All require `Authorization: Bearer <agent_id>:<secret>`.

Categories:

- **Task lifecycle:** `task_done`, `task_failed`, `subtask_create`,
  `task_accept`, `task_request_changes`, `accept_proposal`,
  `reject_proposal`.
- **Knowledge:** `hypothesis_state`, `test_record`,
  `hypothesis_resolve`, `verify`, `observe_world`, `read_artifact`,
  `ask_peer`.
- **World interaction:** `move_intent`, `speak`.
- **Skills (P2.2):** `skill_upsert`, `skill_list`, `skill_attach`,
  `skill_detach`, `skill_delete`.

The full schema for each tool is emitted by `tools/list`. See
`crates/world/src/mcp_http.rs::handle_tools_list` for source of truth.

## Invariants

The 9 ship-gate invariants are documented in
[`docs/superpowers/ship-gate.md`](docs/superpowers/ship-gate.md) with
cross-references to the specific rust tests that prove each.

High-level architectural invariants:

- **Single-writer world state.** All mutations to `WorldView` happen
  on the world's mpsc loop. WS handlers serialize through the loop;
  no shared mutable state across tasks.
- **World owns SQL.** Worker never touches the DB. All SQL via MCP
  tools + HTTP API.
- **Canonical artifact path.** `workspaces/<startup_id>/artifacts/
  <task_id>.md` is enforced by `mcp_dispatch::handle_task_done` —
  rejects any other shape with `bad_artifact_path`.
- **Bearer auth at the edge.** Workers + MCP tool calls both
  authenticate via `<agent_id>:<secret>`. Operators use a separate
  `CLIPTOWN_OPERATOR_TOKEN`.
- **Per-task isolation (P2.3).** Each spawn gets its own
  `<workspaces_root>/workspaces/<sid>/<tid>/workdir/` with an absolute
  symlink back to the workspaces tree.

## Spec references

- [`docs/superpowers/specs/2026-05-07-cliptown-design.md`](docs/superpowers/specs/2026-05-07-cliptown-design.md)
  — original system design.
- [`docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md`](docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md)
  — § 11.9 ship-gate + Phase 2 backlog framing.
- [`docs/superpowers/specs/2026-05-13-phase-3-roadmap.md`](docs/superpowers/specs/2026-05-13-phase-3-roadmap.md)
  — current direction.
- Individual milestone specs at `docs/superpowers/specs/*.md`.
