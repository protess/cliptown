# Real-LLM E2E Runner Design

> Status: **SPEC** (no code yet). Captured 2026-05-09 after exploration session
> found the gap between "M9.10 workflow shell exists" and "§ 11.9 dual-pin
> closed" is three sub-tasks deep, not one.

## Goal

Close ship-gate **§ 11.9** ("All 3 adapters complete a task end-to-end") by
running real LLMs through the worker → adapter → CLI → MCP → world chain in
CI, gated on `workflow_dispatch` with a hard $0.50/run budget cap.

Today the closest thing is `packages/worker/test/contract.test.ts`, which
proves adapters normalize hooks correctly **using a fixture CLI shim** — no
real `claude`/`codex`/`opencode` binary has ever been exercised against the
world. § 11.9's UI proof cell in `docs/superpowers/ship-gate.md` reads
`_real-LLM only — M9.10_` and the workflow YAML at
`.github/workflows/e2e-real-llm.yml` is a placeholder that echoes "runner
lands in Phase 1."

## What exists today (state survey)

This section is the most valuable part of this doc. The session that wrote
it spent its first hour finding these things; the next session should not.

### What's already shipped

- **`.github/workflows/e2e-real-llm.yml`** — `workflow_dispatch`-gated, takes
  `budget_cap_usd` input, exports `E2E_LLM=real` and
  `E2E_BUDGET_CAP_USD`, wires `ANTHROPIC_API_KEY`/`OPENAI_API_KEY` from
  secrets. The runner step is a placeholder: validates the budget input is
  positive and prints "Real-LLM E2E placeholder — runner lands in Phase 1."
- **`packages/adapters/{claude-code,codex,opencode}/`** — adapter packages
  with `spawn()` API. Each writes a per-spawn config dir (`mcp.json`,
  `settings.json`) and starts a hook-bridge HTTP server. Verified by
  `packages/worker/test/contract.test.ts` against a fixture CLI shim
  (`packages/worker/bin/fixture-cli`).
- **`packages/worker/src/mcp.ts`** — `callOverWS` wraps `mcp_call`/`mcp_reply`
  framing on the worker's WS to the world. Correlation IDs, timeouts,
  listener cleanup all tested in `mcp_correlation.test.ts`.
- **`packages/worker/src/llm_mock.ts`** — fixture-driven mock that emits a
  scripted `ToolUse` sequence. Used when worker is launched with `--mock`.
- **The world's MCP dispatch** (`crates/world/src/mcp_dispatch.rs`) handles
  16 tools. Authoritative enumeration:
  ```
  move_intent, speak, task_done, task_failed, subtask_create, task_accept,
  task_request_changes, accept_proposal, reject_proposal, hypothesis_state,
  test_record, hypothesis_resolve, verify, ask_peer, observe_world,
  read_artifact
  ```

### What's missing (the gap)

Three structural gaps surfaced as the design got pulled deeper:

**Gap 1: Worker's main loop has no real-adapter code path.**
`packages/worker/src/main.ts:130-167` — when launched without `--mock` and
without `--fixture`, the worker connects to the WS, prints inbound frames,
and idles. It never calls `claudeCodeAdapter.spawn`. The adapters compile
and pass contract tests, but no production code wires them into the runtime.

**Gap 2: No MCP-over-Unix-socket server exists.**
`packages/adapters/claude-code/src/index.ts:47-57` writes an `mcp.json` that
tells the spawned `claude` CLI to connect to the cliptown MCP server via
`nc -U <mcp_socket_path>` — a Unix-domain-socket stdio MCP transport. But
nothing in `packages/worker/src/` listens on that path. `grep -rln
"net.createServer\|createServer.*unix\|listen.*sock"
packages/worker/src/ packages/adapters/` returns no production matches —
only the `mcp_socket_path` field on adapter configs, which is dangling.

So even with Gap 1 fixed (worker calls `claudeCodeAdapter.spawn`), the
spawned `claude` binary would fail to connect to MCP and either error out
or run blind without tools.

**Gap 3: Worker has no task-loop semantics.**
`packages/worker/src/main.ts:33-44` accepts a single `--prompt` arg passed
at spawn time. There's no notion of "wait for `task_assigned` from world,
then drive the adapter with that task's description." Real-LLM E2E needs
either:
- A one-shot worker (spawn it with a pre-baked prompt; let it run; exit), or
- A task-aware worker loop (worker idles on WS, gets `task_assigned`,
  spawns adapter with task as prompt, on `Stop` emits `task_done`, repeat).

For a single-task E2E proof, the one-shot model is simpler and is what this
spec recommends.

## Sub-task decomposition

> **Architecture decision: MCP-at-the-world (A1').** After inspecting
> multica-ai/multica, cliptown adopts the same pattern: the world hosts
> the MCP server, the CLI authenticates with a per-agent token and hits
> HTTP/SSE directly. The worker becomes a pure process supervisor
> (spawn CLI, log hooks, report exit). See the "References & inspiration"
> section at the bottom for the comparison that drove this choice.

The session that converted this spec to code should ship in this order. Each
sub-task is independently testable and lands as its own PR.

### A1' — MCP-over-HTTP at the world

**Lives at:** `crates/world/src/mcp_http.rs` (new) — axum route + handler.

**Responsibility:** Expose a `/mcp` HTTP endpoint that speaks the MCP
streamable-HTTP transport. Authenticate via Bearer token (same per-agent
secret used by WS worker auth). Route `tools/call` to the existing
`mcp_dispatch::dispatch` function.

**Surface:**

```rust
// In crates/world/src/http.rs (mount alongside existing WS routes):
//   .route("/mcp", post(mcp_http::handle_request))
//   .route("/mcp/sse", get(mcp_http::handle_sse))   // if streamable transport requires it

// Authentication: Bearer token = the agent's WS secret. Look up the agent
// by token, derive AgentView (same shape mcp_dispatch::dispatch expects),
// then forward.
```

**MCP protocol scope:**

The streamable-HTTP transport is the current MCP spec (replaces the older
SSE-only transport). Two routes:
- `POST /mcp` — JSON-RPC request body, JSON response (single-shot).
- `GET /mcp` with `Accept: text/event-stream` — server-sent-events for
  push notifications (we don't need this yet; the 16 cliptown tools are
  all request/response).

For Phase 1 we can ship the POST-only variant. Add SSE later if MCP
notifications become useful.

**Three RPC methods that matter:**

1. **`initialize`** — handshake. Return `protocolVersion`, `capabilities`,
   `serverInfo`. Capabilities: `{ tools: { listChanged: false } }`.
2. **`tools/list`** — enumerate the 16 cliptown tools with JSON Schema
   descriptions. Schemas hand-written in `mcp_http.rs` (or codegen from
   ts-rs types if we want to share with the frontend later).
3. **`tools/call`** — translate `{ name, arguments }` to
   `mcp_dispatch::dispatch(...)` and wrap the reply as
   `{ content: [{ type: "text", text: serde_json::to_string(result) }] }`.

**Build vs. buy:**

The Rust MCP SDK is `rmcp` (https://crates.io/crates/rmcp) — official
Model Context Protocol crate, supports streamable HTTP transport. Recommend
adopting it. If `rmcp` lacks axum integration we can hand-roll the JSON-RPC
shapes — they're flat and well-defined.

**Auth wiring:**

Bearer token validation mirrors the worker WS handshake. Look at
`crates/world/src/http.rs::handle_worker` for the existing secret-based
auth flow. Reuse: same `WORKER_SECRET` env var, same agent-id-from-token
lookup.

**Adapter `mcp.json` change** (touches one TS file):

```diff
// packages/adapters/claude-code/src/index.ts
- function mcpJson(socketPath: string): object {
-   return {
-     mcpServers: {
-       cliptown: { type: "stdio", command: "nc", args: ["-U", socketPath] },
-     },
-   };
- }
+ function mcpJson(worldUrl: string, token: string): object {
+   return {
+     mcpServers: {
+       cliptown: {
+         type: "http",
+         url: `${worldUrl}/mcp`,
+         headers: { Authorization: `Bearer ${token}` },
+       },
+     },
+   };
+ }
```

`SpawnOpts.mcp_socket_path: string` → `mcp_world_url: string, mcp_token: string`
(breaking change to `BackendAdapter`; update opencode + codex adapters to
match, contract tests stay green because they don't exercise MCP wiring).

**Tests** (`crates/world/tests/mcp_http.rs`):

- Spawn the world test fixture.
- `reqwest` client sends `initialize`, asserts handshake.
- Client sends `tools/list`, asserts all 16 names.
- Client sends `tools/call` for `task_done` (with the right token), asserts
  the dispatch ran and returned a result.
- Client sends `tools/call` with a bad token, asserts 401.

**Estimated effort:** 3-4 hours. Most of it is `rmcp` integration learning
+ adapter API breaking change ripple. Slightly more than A1 (Unix socket)
because there are two languages involved (Rust route + TS adapter rewire),
but it pays back in worker simplicity.

**Worker-side simplification (was A1's separate work, now subsumed here):**

Because MCP lives at the world, the worker has no MCP server to maintain.
The worker's only job in real-LLM mode (covered in A2) becomes:
- Read a per-agent token from args/env.
- Spawn the adapter with `mcp_world_url` and `mcp_token`.
- Hook logging.
- Exit when adapter exits.

No Unix socket. No JSON-RPC parsing in Node. Clean.

### A2 — Worker main spawns the adapter

**Edit:** `packages/worker/src/main.ts`.

**Change:** Below the existing `--mock` branch (line 156), add the
real-adapter path:

```typescript
} else if (args.fixture) {
  // existing fixture-CLI path (already in M3.3 contract test wiring)
} else {
  // Real adapter path. Picks the adapter from args.backend.
  const adapter = pickAdapter(args.backend); // returns BackendAdapter
  const spawned = await adapter.spawn({
    prompt: args.prompt,
    cwd: workspaceRoot,
    // MCP lives at the world (A1' decision); pass URL + token, not a socket.
    mcp_world_url: args.worldUrl.replace(/^ws/, "http"),  // ws://host:port → http://host:port
    mcp_token: args.secret,                                // same secret as WS auth
    onHook: (e) => console.log(`[worker] hook: ${e.kind}`),
    onLog: (stream, line) => process[stream === "stderr" ? "stderr" : "stdout"].write(line),
  });
  const exit = await spawned.wait();
  console.log(`[worker] adapter exited code=${exit.exit_code} signal=${exit.signal ?? "none"}`);
  return; // one-shot; worker exits after adapter finishes
}
```

**`pickAdapter`:** new helper that switch-cases on `args.backend`:

```typescript
function pickAdapter(backend: string): BackendAdapter {
  switch (backend) {
    case "claude_code": return claudeCodeAdapter;
    case "codex":       return codexAdapter;       // not in scope this PR
    case "opencode":    return opencodeAdapter;    // not in scope this PR
    default: throw new Error(`unknown backend: ${backend}`);
  }
}
```

For this PR's scope (claude_code first), `codex` and `opencode` cases throw
`not_yet_supported_in_real_mode`. Land that as a TODO comment with a
follow-up note.

**Tests:** No new vitest needed — the existing `main_args.test.ts` already
covers arg parsing. Real-adapter execution is verified by the smoke test in
A3.

**Estimated effort:** 30 minutes. Wiring + URL scheme conversion only.

### A3 — Local smoke test

**Lives at:** `packages/worker/scripts/smoke-real-llm.sh` (new, or
`scripts/` at repo root if that's the convention).

**Pre-requisites the operator must satisfy:**

- `ANTHROPIC_API_KEY` exported.
- `claude` CLI installed and on PATH. Verify with `claude --version`.
  Install via `npm install -g @anthropic-ai/claude-code` (verify package
  name at implementation time; the binary it provides is `claude`).

**Script flow:**

1. Boot world in background: `cargo run --release -p cliptown-world &
   WORLD_PID=$!`. Wait until WS port is listening (poll
   `127.0.0.1:8080/ws/console` or whatever the bind is).
2. Seed via SQL: insert one startup, one engineer agent. Use `sqlite3` on
   the world's database file (the world creates it at a known path — check
   `crates/world/src/config.rs` for the default).
3. Pre-create a queued task assigned to the engineer with a one-line prompt
   ("Write a haiku about clipboards to the canonical artifact path
   `workspaces/<sid>/artifacts/<tid>.md` and call task_done.").
4. Spawn worker: `pnpm -F @cliptown/worker start --
   --world-url=ws://127.0.0.1:8080/ws/worker --agent-id=<engineer>
   --startup-id=<sid> --secret=<known> --backend=claude_code
   --workspace=<tmp> --prompt="<task prompt>"`.
5. Wait for worker to exit (one-shot mode).
6. Verify on disk: `workspaces/<sid>/artifacts/<tid>.md` exists and is
   non-empty.
7. Verify in DB: `SELECT status, artifact_path FROM tasks WHERE id = '<tid>'`
   returns `done` + the canonical path.
8. Verify budget: `SELECT budget_spent_usd FROM startups WHERE id = '<sid>'`
   is `<= $0.50`.
9. Cleanup: `kill $WORLD_PID`, remove tmp dirs.

**Why bash, not TypeScript:** The smoke test is for human-debuggability —
operators run it locally to confirm the pipeline works. Bash makes the
shell-out semantics (process backgrounding, signal handling) explicit.
Sub-task B's CI runner can be TypeScript.

**Estimated effort:** 1 hour to write + 1 hour debugging real LLM
weirdness (rate limits, prompt phrasing, the LLM forgetting to call
task_done, etc.).

### B — CI E2E runner

**Lives at:** `e2e/real-llm/run.ts` (new directory, package as
`@cliptown/e2e-real-llm` workspace or just runs via `tsx`).

**Responsibility:** The script the workflow YAML calls. Same flow as A3 but
TypeScript-native, exits non-zero on any failure, writes structured logs.

**Differences from A3 smoke:**

- Reads `E2E_BUDGET_CAP_USD` from env (fallback `0.50`).
- Writes a JSON result summary to stdout for the workflow to surface.
- No human-friendly pretty-printing — CI logs are machine-grep-targets.

**Estimated effort:** 1-2 hours, mostly translating the bash to TypeScript
with proper error paths.

### C — Workflow YAML

**Edit:** `.github/workflows/e2e-real-llm.yml`.

**Changes to the existing placeholder:**

- Install `claude` CLI: `npm install -g @anthropic-ai/claude-code`.
- Replace the placeholder echo with `pnpm -F @cliptown/e2e-real-llm start`
  (or `tsx e2e/real-llm/run.ts`).
- Add a `matrix:` strategy for the three adapters once codex/opencode are
  wired in their own A2-equivalent passes. For this PR scope, single
  `backend: claude_code`.
- Enforce budget at the CI level too: if the runner reports
  `budget_spent_usd > cap`, fail the job.

**Secrets required (operator task):**

- `ANTHROPIC_API_KEY` in GitHub repo secrets.

For future codex/opencode expansion: `OPENAI_API_KEY` (already wired),
plus whatever opencode uses.

**Estimated effort:** 30 minutes. Mostly fiddling with `npm install -g`
caching.

## Open questions for the next session

The decisions below are deferred. Each has a recommended default but the
session that opens this spec should re-check.

1. **MCP SDK vs hand-roll** — recommended: `@modelcontextprotocol/sdk`. Cost
   is one new dep; benefit is not debugging JSON-RPC framing.

2. **One-shot worker vs task-aware loop** — recommended: one-shot for
   M9.10. Task-aware comes later if the cliptown runtime ever wants to
   reuse a worker across tasks. Keeps this PR contained.

3. **Scenario prompt** — recommended: "Write a haiku about clipboards.
   Save it to your assigned artifact path via the `task_done` MCP tool."
   Constraints: forces the LLM to invoke `task_done` with the canonical
   path (world rejects any other shape with `bad_artifact_path`), keeps
   token use minimal (haiku ≈ 30 output tokens), is non-controversial
   (no policy filter risk).

4. **Hook flow** — recommended: log only. PreToolUse/PostToolUse/Stop
   surface in `[worker] hook: ...` lines; no business logic depends on
   them yet. Future work can drive a state machine here (e.g., "if
   PreToolUse for task_done, mark intended-completion").

5. **Tool schema source of truth** — recommended: hand-write minimal
   schemas at the worker layer for now. Future work could codegen from
   the world's Rust types (similar to how ts-rs already exports
   `ConsoleOutbound` types).

6. **claude CLI version pinning** — recommended: pin a specific version
   in the workflow (`npm install -g @anthropic-ai/claude-code@<v>`) so a
   future CLI release doesn't silently break the E2E.

## Risks

- **Real LLM flakiness.** Claude may decline to call `task_done` or use
  the wrong path. Mitigations: tight scenario prompt, retry-once policy
  in the runner, structured prompt template (system message dictating the
  exact tool call).
- **Cost runaway in CI.** Defense in depth: workflow-level budget input,
  worker-level pause-all at 100% (`crates/world/src/budget.rs:80`),
  runner-level final-check on `budget_spent_usd`. All three should agree.
- **MCP protocol drift.** `@modelcontextprotocol/sdk` is young; breaking
  changes possible. Pin the SDK version in `packages/worker/package.json`.
- **Adapter session length.** `claude --print` is single-prompt
  non-interactive. If the model wants to use multiple tools, it must do so
  within one response. Check whether `--print` allows multi-turn agentic
  loops; if not, switch to interactive mode or use a different invocation.
  This is the highest-uncertainty risk in this design.

## Pre-requisites checklist (operator side)

Before the next coding session can finish A3, the following must be true:

- [ ] `claude` CLI installable via `npm install -g @anthropic-ai/claude-code`
      (verify package name; the spec assumes this but it isn't checked).
- [ ] `ANTHROPIC_API_KEY` available in the operator's shell environment.
- [ ] World server can be run with a known WS bind address (default
      should be fine; document the port in A3 script comments).
- [ ] Operator is willing to spend ~$0.05 per local smoke run.

## Recommended next-session entry point

**Don't re-explore.** Open this doc, jump to A1'.

Concrete entry-point checklist:
1. `git checkout main && git pull && git checkout -b phase-1/m9.10-a1-mcp-at-world`
2. `cargo add rmcp` (verify axum integration; if missing, hand-roll JSON-RPC
   over an axum route — straightforward, the shapes are flat).
3. Write `crates/world/src/mcp_http.rs`: `/mcp` route, Bearer auth from
   existing `WORKER_SECRET` flow, `initialize`/`tools/list`/`tools/call`
   handlers that proxy to `mcp_dispatch::dispatch`.
4. Write `crates/world/tests/mcp_http.rs` with the 4 cases listed.
5. Edit `packages/adapters/claude-code/src/index.ts`: replace `mcpJson`
   to emit `{type: "http", url, headers: {Authorization}}` and update
   `SpawnOpts` (in `@cliptown/adapter-core`) to take `mcp_world_url` +
   `mcp_token` instead of `mcp_socket_path`. Update codex + opencode
   adapters to match (their contract tests don't exercise MCP so they
   stay green).
6. Run `cargo test -p cliptown-world --test mcp_http` and `pnpm test`.
   Iterate until green.
7. Ship A1' as PR 1. Then A2 + A3 (together) as PR 2. Then B as PR 3.
   Then C closes the loop.

## Out of scope

- codex and opencode adapter integration (follow-up PRs once A1-A3 prove
  the architecture works with claude_code).
- Task-aware worker loop (one-shot covers M9.10's invariant).
- Real LLM eval / judge framework (the M9.10 verification is structural:
  did the artifact land at the canonical path, not "is the haiku good").
- Caching `claude` CLI between CI runs (premature optimization until the
  basic flow is green).

## References & inspiration: multica-ai/multica

Inspected during the spec session as a reference implementation of the same
problem class (real coding agents driven by a daemon orchestrator).
Multica's architecture differs from this spec's recommendation in one
non-obvious way that's worth surfacing as an alternative.

### What multica does

- **Go server in the cloud** + **Go daemon on the user's machine**.
- The daemon polls the server for tasks via heartbeat; on assignment, it
  prepares an isolated `execenv` directory (workdir, injected CLAUDE.md /
  AGENTS.md / skills), builds a prompt with `BuildPrompt(task)`, and spawns
  the CLI (`claude`, `codex`) configured with the server-provided
  `task.Agent.McpConfig`.
- **MCP lives at the server, not the daemon.** The `mcp_config` JSON
  delivered to the daemon points the CLI at the multica server's MCP
  endpoint. The CLI authenticates with a token and talks MCP directly to
  the cloud — the daemon is a process supervisor, not a protocol bridge.
- Some tools are also CLI subcommands (`multica issue get/comment/list ...`)
  hitting the server's REST API in parallel with MCP. The agent's prompt
  literally instructs it: "Start by running `multica issue get X --output
  json` to understand your task, then complete it."
- Per-task workspace lives at `{workspacesRoot}/{workspace_id}/{task_id_short}/`.
- See `server/internal/daemon/{daemon.go,prompt.go,types.go}` and
  `server/internal/daemon/execenv/execenv.go`.

### Where multica diverges from cliptown's current design

| Concern | cliptown (current) | multica |
|---|---|---|
| MCP server location | Worker (Unix socket) — see Gap 2 | Server (HTTP/auth) |
| Daemon shape | Per-agent Node worker | Singleton Go daemon, claims many tasks |
| Tool surface | Pure MCP (16 cliptown tools) | MCP + REST CLI subcommands |
| Workspace path | `workspaces/<sid>/artifacts/<tid>.md` (canonical, enforced) | `{root}/{workspace}/{task_short}/` per task |
| Prompt source | Worker `--prompt` arg | Server-built, daemon-injected per task |
| Auth | Worker WS `secret` arg | Per-task token in MCP/REST headers |

### Decision: MCP-at-the-world (A1')

After inspecting multica's daemon orchestrator, cliptown adopts the same
pattern. Details and migration sketch live in the **A1' — MCP-over-HTTP at
the world** sub-task above; this section just captures the trade-off
reasoning so the choice is durable.

**Why MCP-at-world wins for cliptown:**
- One less hop (CLI → world directly, no JSON-RPC ↔ socket ↔ WS bridge).
- Reuses the world's existing Bearer-token auth model.
- No Node-side MCP server to maintain — worker shrinks to process supervisor.
- Matches the deployed-MCP-server pattern most production agents use today.
- `rmcp` (Rust MCP SDK) has HTTP transport built-in; less custom code than
  a from-scratch Unix-socket JSON-RPC implementation in Node.

**What we give up:**
- World's HTTP surface area grows (auth, CORS, rate limits in one more
  place). Mitigation: reuse existing axum middleware, mirror WS auth flow.
- Adapter API breaking change (`mcp_socket_path` → `mcp_world_url + mcp_token`).
  Mitigation: only 3 adapter packages; contract tests don't exercise MCP
  so they stay green.

### What we explicitly do NOT take from multica

- **REST CLI subcommands on top of MCP.** Multica has `multica issue get`,
  `multica issue comment list`, etc. — a parallel tool surface. Cliptown's
  16 tools fit MCP cleanly; no dual surface needed.
- **Heartbeat task-claiming protocol.** Multica's daemon polls; cliptown's
  world pushes via WS frames. Don't change this.
- **pgvector dependency.** Multica uses Postgres + pgvector for semantic
  search. Cliptown's SQLite is enough until we have a use case demanding
  vectors (not for M9.10).

## Phase 2 backlog (post-M9.10): multica patterns worth importing

Three patterns from multica are worth pulling in but aren't blockers for
§ 11.9. Each gets its own spec when it lands. Listed in priority order.

### P2.1 — Daemon health buckets for worker monitoring

**Multica reference:**
`packages/core/runtimes/types.ts` and
`packages/core/runtimes/derive-health.ts`.

**The pattern:** worker liveness in 4 time-bucketed states instead of
binary online/offline:
- `online` — heartbeat within threshold
- `recently_lost` — offline < 5 minutes (likely transient, hide from alerts)
- `offline` — offline 5 minutes to 7 days
- `about_to_gc` — within 1 day of the 7-day GC threshold

**Why import:** cliptown today flips workers between "connected to WS" and
"WS closed." A 5-minute network blip looks identical to a hard crash to
the operator console, which is noisy. Bucketed health collapses transient
noise and makes "this worker is actually dead, clean it up" surface only
when it should.

**Where it lives in cliptown:**
- World tracks last heartbeat per agent (already in `state.rs` avatars,
  probably).
- New module `crates/world/src/health.rs` derives the 4-state from
  `now - last_heartbeat_at` with the same thresholds.
- TaskVM / AvatarVM gain a `health: "online" | "recently_lost" | ...` field.
- Console UI dims/marks workers in the non-online states.

**Effort:** ~2-3 hours. Pure additive — no behavior changes, just an enum
+ derive + UI surface.

**Land after M9.10 because:** it's monitoring polish, not E2E plumbing.

### P2.2 — Skills system

**Multica reference:**
- `server/internal/daemon/execenv/execenv.go` (`SkillContextForEnv` +
  `Files`).
- `server/internal/daemon/local_skills.go` for the daemon-side report.
- `apps/web/features/skills/` (UI).

**The pattern:** reusable per-task capabilities stored as
`{name, content (markdown), files []}`. At spawn time, the daemon writes
the skill content and supporting files into the agent's working directory
so the CLI's filesystem context includes them. Skills can be workspace-
scoped or global; agents can be assigned a subset.

Example: a "deploy-to-fly" skill = the markdown describing the runbook +
a `fly.toml` template file + a `Dockerfile.example`. Agent gets all three
written to its workdir before the CLI starts; prompt mentions the skill
by name.

**Why import:** cliptown's agents currently only see `task.title` +
`task.description`. No way to compound reusable capability. Each new agent
session reinvents the wheel. Skills give "this agent knows how to do X"
a concrete artifact form.

**Where it lives in cliptown:**
- New DB table: `skills (id, startup_id, name, content_md, files_json, ...)`.
- New table: `agent_skills (agent_id, skill_id)` — many-to-many.
- World snapshot includes attached skills per agent.
- Worker writes skill content + files into the per-task workspace at
  spawn time (depends on P2.3 below).
- Frontend skill management UI (list, edit, attach).
- ConsoleOutbound add-on: `skill_changed` notifications.

**Effort:** ~1-2 weeks. Real product surface area — schema, UI, lifecycle,
permissioning. Pre-req: P2.3 (execenv) to have somewhere to write files.

**Land after M9.10 because:** product feature, not infrastructure. Needs
its own design conversation about scope (workspace-scoped only? agent
templates? versioning?).

### P2.3 — Per-task execenv directories

**Multica reference:**
`server/internal/daemon/execenv/execenv.go`.

**The pattern:** at task assignment, the daemon creates
`{workspacesRoot}/{workspace_id}/{task_id_short}/` containing:
- `workdir/` — what `Cwd` is set to when CLI spawns.
- Injected `CLAUDE.md` / `AGENTS.md` with task + agent context.
- Skill content unpacked (P2.2 dependency).
- Repo checkouts (cloned in-band as needed).

GC sweeps old dirs after 7 days.

**Why import:** cliptown's worker currently passes a flat `--workspace`
arg. There's no isolation between tasks running on the same agent; no
place to inject context files; no automatic cleanup. As the system grows
beyond toy scenarios this gap will bite.

**Where it lives in cliptown:**
- Worker's `--workspace` becomes `--workspaces-root`.
- On adapter spawn, worker creates `<root>/<startup_id>/<task_id_short>/`
  with `workdir/`, writes `CLAUDE.md` with task + agent context, writes
  any attached skill files.
- GC daemon (separate concern, low priority) sweeps after 7 days.
- The canonical artifact path stays world-enforced
  (`workspaces/<sid>/artifacts/<tid>.md`); execenv lives alongside, not
  in conflict.

**Effort:** ~3-5 days. Schema-light but lots of paths to update.

**Land after M9.10 because:** depends on having a real-LLM flow to even
need per-task isolation. Until we have real agents running real tasks,
all of this scaffolding is hypothetical.
