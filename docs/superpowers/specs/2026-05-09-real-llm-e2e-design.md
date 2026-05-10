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

The session that converted this spec to code should ship in this order. Each
sub-task is independently testable and lands as its own PR.

### A1 — MCP-over-Unix-socket server in worker

**Lives at:** `packages/worker/src/mcp_socket_server.ts` (new).

**Responsibility:** Listen on a Unix socket; speak JSON-RPC 2.0 MCP protocol;
proxy `tools/call` to the world via `callOverWS`. Lifecycle owned by the
worker.

**Surface:**

```typescript
export interface McpSocketServer {
  socketPath: string;       // tmp socket path the adapter will be told to use
  close(): Promise<void>;   // unlinks socket + closes all client connections
}

export async function startMcpSocketServer(
  ws: WorkerHandle,
  opts?: { socketPath?: string },
): Promise<McpSocketServer>;
```

**MCP protocol scope:**

The Model Context Protocol for stdio transport uses newline-delimited JSON
(per the latest spec — older drafts used `Content-Length` headers like LSP;
verify against `@modelcontextprotocol/sdk` source at implementation time).
The three RPC methods that matter:

1. **`initialize`** — handshake. Return `protocolVersion`, `capabilities`,
   `serverInfo`. Spec capabilities: `{ tools: { listChanged: false } }`.
2. **`tools/list`** — enumerate the 16 cliptown tools with JSON Schema
   descriptions. The schemas live in the world's MCP dispatch but are not
   exposed; this server must define them. Suggestion: hand-write minimal
   schemas (description + required args) at the worker layer; precise
   validation is the world's job anyway.
3. **`tools/call`** — translate `{ name, arguments }` to
   `callOverWS(ws, { tool: name, args: arguments })` and wrap the reply as
   `{ content: [{ type: "text", text: JSON.stringify(result) }] }`.

**Build vs. buy:**

`@modelcontextprotocol/sdk` ships a TypeScript reference implementation
(`Server` class, `StdioServerTransport`, JSON-RPC handling). **Recommend
buying** — hand-rolling JSON-RPC framing for one consumer is YAGNI. The SDK
is ~30KB and zero-dep aside from `zod`.

The SDK's `StdioServerTransport` reads stdin/writes stdout. For Unix-socket
transport, the SDK's `Server` decouples from transport; we'd write a tiny
`UnixSocketServerTransport` that plays the same role. Or use a custom MCP
transport — research at implementation time.

**Tests** (`packages/worker/test/mcp_socket_server.test.ts`):

- Vitest spawns a fake `WorkerHandle` (mock WS).
- Test client connects to the socket, sends `initialize`, expects handshake.
- Test client sends `tools/list`, expects all 16 tool names.
- Test client sends `tools/call` for `task_done`, mock WS receives a frame
  with the right corr_id, server returns the mocked reply wrapped.
- Test client sends `tools/call` for an unknown tool, expects an error.

**Estimated effort:** 2-3 hours (most of it learning the MCP SDK + handling
the transport quirk).

### A2 — Worker main spawns the adapter

**Edit:** `packages/worker/src/main.ts`.

**Change:** Below the existing `--mock` branch (line 156), add a `else`
branch:

```typescript
} else if (args.fixture) {
  // existing fixture-CLI path (already in M3.3 contract test wiring)
} else {
  // Real adapter path. Picks the adapter from args.backend.
  const adapter = pickAdapter(args.backend); // returns BackendAdapter
  const mcpServer = await startMcpSocketServer(handle);
  try {
    const spawned = await adapter.spawn({
      prompt: args.prompt,
      cwd: workspaceRoot,
      mcp_socket_path: mcpServer.socketPath,
      onHook: (e) => console.log(`[worker] hook: ${e.kind}`),
      onLog: (stream, line) => process[stream === "stderr" ? "stderr" : "stdout"].write(line),
    });
    const exit = await spawned.wait();
    console.log(`[worker] adapter exited code=${exit.exit_code} signal=${exit.signal ?? "none"}`);
  } finally {
    await mcpServer.close();
  }
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

**Estimated effort:** 30 minutes. Mostly wiring.

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

**Don't re-explore.** Open this doc, jump to A1.

Concrete entry-point checklist:
1. `git checkout main && git pull && git checkout -b phase-1/m9.10-a1-mcp-socket-server`
2. `pnpm add -F @cliptown/worker @modelcontextprotocol/sdk`
3. Write `packages/worker/src/mcp_socket_server.ts` per the A1 surface above.
4. Write `packages/worker/test/mcp_socket_server.test.ts` with the 4 test
   cases listed.
5. Run vitest; iterate until green.
6. Ship A1 as PR 1. Then A2 + A3 (probably together) as PR 2. Then B as
   PR 3. Then C closes the loop.

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

### Alternative architecture: MCP-at-the-world (A1' instead of A1)

The single biggest architectural decision multica forces re-examination of:
**should the MCP server live at the worker (Unix socket) or at the world
(HTTP endpoint)?**

A1' sketch:
- Add an `/mcp` HTTP/SSE route to the world (axum). Use the streamable HTTP
  transport from `@modelcontextprotocol/sdk` (or `rmcp` on the Rust side).
- The route accepts a Bearer token (per-agent secret, same one the worker
  uses for WS auth) and routes `tools/call` straight to the existing
  `mcp_dispatch::dispatch` function.
- Adapter's `mcp.json` becomes `{ type: "http", url:
  "http://world.local:PORT/mcp", headers: { Authorization: "Bearer ..." } }`.
- The Unix socket disappears. The worker becomes a pure process supervisor
  (spawn CLI, log hooks, report exit).

**Why this might be better:**
- One less hop. CLI → world directly, no JSON-RPC ↔ socket ↔ WS bridge.
- Reuses the world's existing auth model.
- No Node-side MCP server to maintain.
- Matches the deployed-MCP-server pattern most production agents use today.
- The MCP SDK has solid HTTP/SSE transport; less surface area than custom
  Unix-socket framing.

**Why the worker-hosts-MCP design (A1) might still be better:**
- All cliptown traffic already flows through the worker WS. Adding HTTP
  expands the world's surface area (auth, CORS, rate limits, observability).
- The worker is on the same machine as the CLI; Unix socket has lower
  overhead than HTTP+TLS+token-validation.
- The current adapter already writes `nc -U <socket>` configs; switching
  to HTTP changes the adapter API (`mcp_socket_path: string` →
  `mcp_url: string`).
- Cliptown's runtime model assumes per-agent workers — there's no
  "one daemon, many tasks" abstraction to leverage as multica has.

**Recommendation for the next session: re-decide before implementing A1.**

Re-decide A1 vs A1' as the first thing in the next session. The 30 minutes
spent comparing now will save 2-3 hours if we choose the wrong direction.
Read both this section and `server/internal/daemon/execenv/runtime_config.go`
in the multica repo to see the multica MCP config shape concretely, then
commit to one architecture. The rest of this spec assumes A1; if the
choice flips, A2/A3 stay largely the same (just transport details change).

### What we explicitly do NOT take from multica

- **REST CLI subcommands on top of MCP.** Multica has `multica issue get`,
  `multica issue comment list`, etc. — a parallel surface to MCP. Cliptown
  doesn't need this dual surface: all 16 cliptown tools fit MCP cleanly,
  and the agent prompt can describe them in one place.
- **Heartbeat task-claiming protocol.** Multica's daemon polls for tasks;
  cliptown's world pushes via WS frames. Don't change this.
- **Per-task workspace via `execenv`.** Cliptown's canonical artifact path
  is enforced server-side; we don't need the daemon to build a workspace
  scaffold. The world's sandbox check already rejects bad paths.
