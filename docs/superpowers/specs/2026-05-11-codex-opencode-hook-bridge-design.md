# codex + opencode hook bridge — design

**Date:** 2026-05-11
**Status:** shipped
**Driver:** Phase 0 known-limitation cleanup. Today both `codex` and `opencode`
adapters advertise `hooks: [session_stop, session_error]` but **none of the
four hook events actually flow**. Each adapter spins up an HTTP `startHookBridge`
listener and exposes its port via `CODEX_HOOK_PORT` / `OPENCODE_HOOK_PORT`,
but nothing on the upstream CLI side ever POSTs to it — the bridge is dead
weight. claude-code is the only adapter where hooks reach the worker, via
the `settings.json` `PreToolUse`/`PostToolUse`/`Stop`/`Notification` hook
script contract.

This spec brings codex and opencode to parity by ingesting each CLI's
**native event stream** in the adapter process and synthesizing `HookEvent`s
directly into `opts.onHook`. The HTTP bridge stays for claude-code (it is
the right model for that CLI), and is removed from codex/opencode where it
was never wired.

## Goals

- codex spawns produce `pre_tool` and `post_tool` hooks on every tool call
  (`command_execution` and `mcp_tool_call`), plus `session_stop` /
  `session_error` at termination.
- opencode spawns produce `pre_tool` and `post_tool` on every tool call
  with **honest pre/post semantics** (pre fires before the tool runs,
  not synthesized from a completed event), plus `session_stop` /
  `session_error`.
- Capability advertising matches reality:
  `hooks = [pre_tool, post_tool, session_stop, session_error]` for both.
- All three adapters still complete the § 11.9 real-LLM smoke
  (`workspaces/<sid>/artifacts/<tid>.md` produced, task transitions to
  `awaiting_review`, budget telemetry forwarded).
- No new HookKind values; no new MCP surface; no worker-side changes
  beyond the existing `console.log` hook consumer.

## Non-goals (explicit)

- New HookKind values (`turn_started`, `thread_started`, etc.).
- Worker-side structural consumption of hooks (sandbox / policy / audit).
  Worker still just `console.log`s them.
- Frontend / operator-console hook visualization.
- Changes to § 11.9 smoke scripts or the contract test fixture CLI's
  claude-code path.
- opencode authentication (server runs unsecured on 127.0.0.1 random
  port — same trust model as the existing hook bridge).

## Architecture

Two adapters, two different paths driven by what the upstream CLI exposes.

### codex — stdout JSONL → onHook

`codex exec --json` emits a JSONL stream on stdout. Each line is a typed
event: `thread.started`, `turn.started`, `item.started`, `item.completed`,
`turn.completed`. Tool calls (`command_execution`, `mcp_tool_call`) get
distinct `started` / `completed` items, giving clean pre/post semantics.

The adapter already buffers stdout for `parseUsage`. We refactor that into
a **per-line streaming parser** that drives two consumers in lockstep:

1. **hook emitter** — translates `item.started` / `item.completed` /
   process exit into `HookEvent`s, calls `opts.onHook` per event.
2. **usage accumulator** — sums `turn.completed.usage` (existing behavior).

No HTTP bridge involvement. `startHookBridge` call and `CODEX_HOOK_PORT`
env are removed.

### opencode — server mode + SSE → onHook

`opencode run --format json` only emits `tool_use` events with
`state.status: "completed"` (tool already done by the time we see it).
There is no way to get a "tool starting" event from `opencode run`.

The `opencode serve` headless server, however, exposes a `/event` SSE
stream where `message.part.updated` events fire on every state
transition: `pending → running → completed` for tool parts. This gives
true pre/post semantics.

So the opencode adapter is rebuilt around server mode:

1. Write `opencode.json` to `opts.cwd` (existing logic, unchanged).
2. Spawn `opencode serve --port 0 --pure --hostname 127.0.0.1` as a
   child process. Parse `opencode server listening on http://127.0.0.1:<port>`
   from stderr to capture the chosen port. 15-second timeout.
3. Health probe `GET /global/health`.
4. Open SSE subscription to `GET /event`. Line-buffered `data: ` parser
   over a Node `fetch` ReadableStream.
5. `POST /session` with header `x-opencode-directory: <opts.cwd>` →
   sessionID.
6. `POST /session/<sid>/message` with body
   `{parts:[{type:"text",text:opts.prompt}], agent:"build", model:{providerID,modelID}}`.
   The response resolves immediately; real progress lands on the SSE
   stream.
7. SSE consumer maps events to `HookEvent`s (table below). On
   `session.idle`, emit `session_stop`, `DELETE /session/<sid>`
   best-effort, `SIGTERM` the server child, close SSE.
8. `wait()` resolves with the server child's exit code plus accumulated
   `UsageReport`.

`startHookBridge` call and `OPENCODE_HOOK_PORT` env are removed.
`opencode run` invocation is replaced entirely by the serve + REST/SSE
flow.

### claude-code — unchanged

`startHookBridge` stays in `adapter-core`; claude-code adapter keeps
calling it. The `settings.json` `curl --data-binary @-` hook script
contract is exactly what claude-code expects, so the existing model is
right for that CLI.

## Event mapping

### codex JSONL → HookEvent

| JSONL line | HookKind | tool | payload | notes |
|---|---|---|---|---|
| `thread.started` | — (skip) | — | — | no equivalent HookKind |
| `turn.started` | — (skip) | — | — | turn boundary; future work |
| `item.started`, type=`command_execution` | `pre_tool` | `"shell"` | full item object | normalize shell commands to a single tool name (cliptown convention) — raw command goes in payload |
| `item.started`, type=`mcp_tool_call` | `pre_tool` | `item.tool_name` (or `"mcp"` fallback) | full item object | exact tool-name key TBD at first capture; pinned via fixture |
| `item.started`, type=`agent_message` | — (skip) | — | — | text output, not a tool |
| `item.completed`, type=`command_execution` | `post_tool` | `"shell"` | full item (exit_code, output) | |
| `item.completed`, type=`mcp_tool_call` | `post_tool` | `item.tool_name` | full item (result/error) | |
| `item.completed`, type=`agent_message` | — (skip) | — | — | |
| `turn.completed` | — (skip) | — | — | usage accumulated separately |
| process exit, code=0 | `session_stop` | `""` | `{exit_code:0, usage?}` | after stream end |
| process exit, code≠0 | `session_error` | `""` | `{exit_code, signal?, stderr_tail}` | last ~16 lines of stderr |

`seq` is a monotonic counter local to the adapter spawn (matching
`hook_bridge.ts` semantics).

### opencode SSE → HookEvent

The `/event` SSE stream emits multiple event types; we care about a
subset. Per-callID dedup is required because `message.part.updated`
with `state.status=running` can fire multiple times for a single tool
call (progress updates).

| SSE event | sub-shape | HookKind | tool | notes |
|---|---|---|---|---|
| `server.connected` | — | skip | | initial frame |
| `server.heartbeat` | — | skip | | keepalive |
| `session.created` | — | skip | | |
| `session.updated` | — | skip | | top-level session state |
| `session.status` | (terminal error variant) | `session_error` | `""` | exact shape pinned at first observed failure |
| `session.idle` | — | `session_stop` | `""` | terminal signal — triggers teardown |
| `session.diff` | — | skip | | |
| `message.updated` | — | skip | | top-level message state |
| `message.part.delta` | — | skip | | text streaming chunks |
| `message.part.updated` | part.type=`step-start` | skip | | |
| `message.part.updated` | part.type=`step-finish` | skip | | tokens/cost extracted into UsageReport here |
| `message.part.updated` | part.type=`text` / `reasoning` | skip | | |
| `message.part.updated` | part.type=`tool`, state.status=`pending` | skip | | input not yet finalized |
| `message.part.updated` | part.type=`tool`, state.status=`running`, callID first seen | `pre_tool` | `part.tool` | payload = `part.state.input` |
| `message.part.updated` | part.type=`tool`, state.status=`running`, callID already fired pre | skip | | progress update |
| `message.part.updated` | part.type=`tool`, state.status=`completed` / `failed`, callID first terminal | `post_tool` | `part.tool` | payload = `part.state.output` + `part.state.time` |
| `message.part.updated` | part.type=`tool`, state.status=terminal, callID already fired post | skip | | duplicate frame |

**Dedup state:** `Map<callID, { fired_pre: boolean, fired_post: boolean }>`,
local to the spawn.

**Usage accumulation:** `step-finish.part.tokens` and `part.cost` are
summed across all step-finish events into the existing `UsageReport`
shape. opencode reports cost in USD, so `cost_usd` is populated
directly (current behavior preserved).

## File structure

### `packages/adapters/codex/src/`

- `index.ts` — spawn entry point; process lifecycle and stdout line
  splitter. Calls into `event_parser` per line.
- `event_parser.ts` *(new)* — pure function. Input: one JSONL line +
  prior accumulator state. Output: `{ hooks: HookEvent[], usage_delta }`.
  No I/O; trivially unit-testable.

### `packages/adapters/opencode/src/`

- `index.ts` — spawn entry point; orchestrates serve lifecycle, REST,
  SSE, hook forwarding.
- `serve_lifecycle.ts` *(new)* — `startServer(opts) → { port, ready,
  kill }`. stderr line parser captures listening URL.
- `sse_client.ts` *(new)* — line-buffered SSE consumer over Node fetch
  ReadableStream. `data: ` prefix stripping, `\n\n` event boundary,
  abort-signal cleanup.
- `event_mapper.ts` *(new)* — pure function. Input: one SSE event JSON +
  callID map. Output: `HookEvent[]` + usage delta.
- `session_client.ts` *(new)* — thin fetch wrapper for `POST /session`,
  `POST /session/<sid>/message`, `DELETE /session/<sid>`.

### `packages/adapters/core/src/`

- `hook_bridge.ts` — unchanged. Still used by claude-code.

## Capability advertising

| adapter | hooks before | hooks after | inject_context | block_on_stop |
|---|---|---|---|---|
| claude-code | `[pre_tool, post_tool, session_stop, session_error]` | (unchanged) | true | true |
| codex | `[session_stop, session_error]` (advertised, none actually flowed) | `[pre_tool, post_tool, session_stop, session_error]` | false | false |
| opencode | `[session_stop, session_error]` (advertised, none actually flowed) | `[pre_tool, post_tool, session_stop, session_error]` | true | false |

## Test strategy

### Unit tests

**codex (`packages/adapters/codex/test/`):**

- `hooks.test.ts` — existing shape test, updated to assert
  `hooks` now contains all four kinds.
- `event_parser.test.ts` *(new)* — fixture JSONL strings drive
  `parseLine` line by line. Cases:
  - shell tool: `thread.started → turn.started → item.started(command_execution) → item.completed(command_execution) → item.completed(agent_message) → turn.completed` → `[pre_tool shell, post_tool shell]`.
  - mcp tool: same shape with `mcp_tool_call` items, tool name matches `item.tool_name`.
  - usage delta sums correctly across two turns.
  - malformed line → no crash, no emit.

**opencode (`packages/adapters/opencode/test/`):**

- `hooks.test.ts` — existing shape test, updated for new caps.
- `event_mapper.test.ts` *(new)* — fixture SSE event JSON drives
  `mapEvent`. Cases:
  - tool lifecycle `bash pending → bash running → bash running → bash running → bash completed` → emits exactly `[pre_tool bash, post_tool bash]`.
  - two concurrent tools with different callIDs interleaved → each gets one pre and one post.
  - `session.idle` → `session_stop`.
  - unknown event type → no emit, no crash.
- `sse_client.test.ts` *(new)* — in-process `http.createServer` emits
  `data: {...}\n\n` frames; client parses and yields events; abort
  signal closes cleanly.

### Contract tests

`packages/adapters/contract/` runs each adapter against the fixture CLI.
The fixture CLI assumes claude-code's `settings.json` hook-script model,
which codex and opencode no longer use. Decision: **fixture branch in
each adapter directly synthesizes a hook sequence via `opts.onHook`
when `CLIPTOWN_FIXTURE_CLI` is set**, instead of teaching fixture-cli
to speak codex JSONL or opencode SSE.

- In codex `index.ts`: when `isFixture`, after spawning the fixture CLI
  (which still produces the prompt+exit path), the adapter emits a
  synthetic `[pre_tool shell, post_tool shell, session_stop]`
  sequence into `onHook` so contract tests see the new caps in action.
- Same for opencode.

This keeps the contract suite passing without coupling fixture-cli to
two new CLI dialects.

### Integration

§ 11.9 real-LLM smoke covers true end-to-end. One pass against each of
the three backends after merge. Cost is already-budgeted (smoke runs
already cost ~$0.5 per pass).

## Migration / risk

### Removed code

- codex `index.ts`: `bridge` variable, `startHookBridge` call,
  `CODEX_HOOK_PORT` env, finally-block bridge cleanup.
- opencode `index.ts`: `bridge` variable, `startHookBridge` call,
  `OPENCODE_HOOK_PORT` env, plus the entire `opencode run` invocation
  (replaced by serve + REST/SSE).

### Preserved code

- `adapter-core/hook_bridge.ts` — still used by claude-code.
- `opencode.json` writer in opencode adapter — same content, same
  location.
- `CODEX_MODEL_ID`, `OPENCODE_MODEL`, `CLIPTOWN_MCP_TOKEN` env handling
  — unchanged.

### Runtime characteristics

- No new runtime deps. Node 18+ built-ins (`fetch`, ReadableStream)
  cover SSE consumption.
- `opencode serve` cold start ≈ 3s; adds ~3s latency to every opencode
  spawn. Acceptable inside § 11.9 smoke envelope.
- `opencode serve` resident memory ≈ 150MB (Bun runtime). Single
  smoke startup is fine; running many concurrent opencode workers
  would multiply this. Out of scope for now.

### Upstream compatibility risk

- opencode 1.4.x API surface (POST `/session`, `/event` SSE,
  `message.part.updated` shape) is not formally versioned. cliptown
  already pins opencode 1.4.x; same policy. opencode 1.5.x or newer
  requires re-verification.
- codex `mcp_tool_call` item shape (`tool_name` field key) was not
  captured live — first implementation captures it via a probe and
  pins the exact key in `event_parser.test.ts` fixture.

## Definition of done

- `cargo test -p cliptown-world` (~219) — green, unchanged.
- `pnpm -F @cliptown/worker test` (~63) — green, unchanged.
- `pnpm -F @cliptown/adapter-core test` — green, unchanged.
- `pnpm -F @cliptown/adapter-claude-code test` — green, unchanged.
- `pnpm -F @cliptown/adapter-codex test` — green, plus new
  `event_parser.test.ts`.
- `pnpm -F @cliptown/adapter-opencode test` — green, plus new
  `event_mapper.test.ts` + `sse_client.test.ts`.
- `pnpm -F @cliptown/adapter-contract test` (12) — green via fixture
  hook-synthesis branch.
- `pnpm -F @cliptown/frontend test` (14 e2e) — green, unchanged.
- § 11.9 smoke: claude-code / codex / opencode each produces
  `awaiting_review` artifact end-to-end.
- worker log during smoke shows non-empty `pre_tool` / `post_tool`
  lines for all three backends (visual confirmation of the change).
