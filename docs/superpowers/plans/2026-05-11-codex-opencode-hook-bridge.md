# codex + opencode hook bridge — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring codex + opencode adapters to hook parity with claude-code: `pre_tool` / `post_tool` / `session_stop` / `session_error` actually flow to `opts.onHook` for every backend.

**Architecture:** codex ingests its native `--json` JSONL stdout via an in-adapter streaming parser; opencode is rebuilt around `opencode serve` + REST/SSE so we can observe `message.part.updated` state transitions (true pre/post semantics). The dead HTTP `startHookBridge` listener is removed from both; it stays on claude-code where the CLI's `settings.json` hook script contract actually uses it.

**Tech Stack:** TypeScript, Node 18+ (`fetch` + ReadableStream), vitest, pnpm workspaces. No new external deps.

**Spec:** `docs/superpowers/specs/2026-05-11-codex-opencode-hook-bridge-design.md`

---

## File structure

### codex
- `packages/adapters/codex/src/event_parser.ts` *(new)* — pure streaming JSONL → `HookEvent[]` + usage accumulator.
- `packages/adapters/codex/src/index.ts` *(modify)* — remove `startHookBridge`/`CODEX_HOOK_PORT`; wire stdout chunks into parser; add fixture-branch synthetic hook emission; replace `parseUsage` with state-based readout.
- `packages/adapters/codex/test/event_parser.test.ts` *(new)* — fixture JSONL → HookEvent assertions.
- `packages/adapters/codex/test/hooks.test.ts` *(modify)* — update caps expectation to all four hooks.

### opencode
- `packages/adapters/opencode/src/sse_client.ts` *(new)* — Node fetch + ReadableStream → async iterator of `data: ` events.
- `packages/adapters/opencode/src/event_mapper.ts` *(new)* — pure SSE event JSON + callID map → `HookEvent[]` + usage accumulator.
- `packages/adapters/opencode/src/serve_lifecycle.ts` *(new)* — spawn `opencode serve`, parse listening port from stderr, expose `kill()`.
- `packages/adapters/opencode/src/session_client.ts` *(new)* — thin fetch wrappers for `POST /session`, `POST /session/<sid>/message`, `DELETE /session/<sid>`.
- `packages/adapters/opencode/src/index.ts` *(rewrite)* — orchestrate the four modules; remove `startHookBridge`/`OPENCODE_HOOK_PORT`; replace `opencode run` flow entirely; add fixture-branch synthetic hook emission.
- `packages/adapters/opencode/test/event_mapper.test.ts` *(new)* — fixture SSE event JSON → HookEvent assertions.
- `packages/adapters/opencode/test/sse_client.test.ts` *(new)* — in-process SSE server → client parses events correctly.
- `packages/adapters/opencode/test/hooks.test.ts` *(modify)* — update caps expectation to all four hooks.

### contract / smoke
- `packages/worker/test/contract.test.ts` — no code change required; the cross-adapter M8.3 `it.each` block already asserts capability shape and will pick up the new `pre_tool`/`post_tool` entries via the new test assertions in each adapter's `hooks.test.ts`.

---

## Task 1: Codex event_parser — failing tests

**Files:**
- Test: `packages/adapters/codex/test/event_parser.test.ts`

- [ ] **Step 1: Create the failing test file**

```ts
// packages/adapters/codex/test/event_parser.test.ts
import { describe, it, expect } from "vitest";
import { emptyState, parseChunk, finalize } from "../src/event_parser.js";

describe("codex event_parser", () => {
  it("emits pre_tool then post_tool for a command_execution item", () => {
    const state = emptyState();
    const lines = [
      `{"type":"thread.started","thread_id":"t1"}`,
      `{"type":"turn.started"}`,
      `{"type":"item.started","item":{"id":"item_0","type":"command_execution","command":"/bin/zsh -lc ls","aggregated_output":"","exit_code":null,"status":"in_progress"}}`,
      `{"type":"item.completed","item":{"id":"item_0","type":"command_execution","command":"/bin/zsh -lc ls","aggregated_output":"out\\n","exit_code":0,"status":"completed"}}`,
      `{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"done"}}`,
      `{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":50,"output_tokens":20}}`,
      "",
    ].join("\n");
    const { hooks } = parseChunk(lines, state);
    expect(hooks.map((h) => [h.kind, h.tool])).toEqual([
      ["pre_tool", "shell"],
      ["post_tool", "shell"],
    ]);
    expect(hooks[0].payload).toMatchObject({ type: "command_execution", status: "in_progress" });
    expect(hooks[1].payload).toMatchObject({ type: "command_execution", status: "completed", exit_code: 0 });
    expect(hooks[0].seq).toBe(1);
    expect(hooks[1].seq).toBe(2);
    expect(state.usage).toEqual({ in_tokens: 150, out_tokens: 20, saw: true });
  });

  it("emits pre_tool/post_tool for mcp_tool_call using item.tool field", () => {
    const state = emptyState();
    const lines = [
      `{"type":"item.started","item":{"id":"i0","type":"mcp_tool_call","server":"cliptown","tool":"mcp__cliptown__task_done","status":"in_progress"}}`,
      `{"type":"item.completed","item":{"id":"i0","type":"mcp_tool_call","server":"cliptown","tool":"mcp__cliptown__task_done","result":{"ok":true},"status":"completed"}}`,
      "",
    ].join("\n");
    const { hooks } = parseChunk(lines, state);
    expect(hooks.map((h) => h.kind)).toEqual(["pre_tool", "post_tool"]);
    expect(hooks[0].tool).toBe("mcp__cliptown__task_done");
    expect(hooks[1].tool).toBe("mcp__cliptown__task_done");
  });

  it("falls back to 'mcp' when mcp_tool_call lacks a tool field", () => {
    const state = emptyState();
    const line = `{"type":"item.started","item":{"id":"i0","type":"mcp_tool_call","status":"in_progress"}}` + "\n";
    const { hooks } = parseChunk(line, state);
    expect(hooks[0].tool).toBe("mcp");
  });

  it("skips agent_message and turn boundaries", () => {
    const state = emptyState();
    const lines = [
      `{"type":"thread.started","thread_id":"t1"}`,
      `{"type":"item.completed","item":{"id":"i9","type":"agent_message","text":"hi"}}`,
      `{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}`,
      "",
    ].join("\n");
    const { hooks } = parseChunk(lines, state);
    expect(hooks).toEqual([]);
  });

  it("handles a chunk split mid-line via carry buffer", () => {
    const state = emptyState();
    const first = `{"type":"item.started","item":{"id":"i0","ty`;
    const second = `pe":"command_execution","command":"x","aggregated_output":"","exit_code":null,"status":"in_progress"}}\n`;
    const a = parseChunk(first, state);
    const b = parseChunk(second, state);
    expect(a.hooks).toEqual([]);
    expect(b.hooks).toHaveLength(1);
    expect(b.hooks[0].kind).toBe("pre_tool");
    expect(b.hooks[0].tool).toBe("shell");
  });

  it("silently skips malformed JSON lines", () => {
    const state = emptyState();
    const lines = [
      `not json`,
      `{"type":"item.started","item":{"id":"i0","type":"command_execution","status":"in_progress"}}`,
      "",
    ].join("\n");
    const { hooks } = parseChunk(lines, state);
    expect(hooks).toHaveLength(1);
  });

  it("finalize emits session_stop on exit 0", () => {
    const state = emptyState();
    parseChunk(`{"type":"turn.completed","usage":{"input_tokens":7,"output_tokens":3}}\n`, state);
    const { hooks } = finalize(state, { exit_code: 0, signal: undefined, stderr_tail: "" });
    expect(hooks).toHaveLength(1);
    expect(hooks[0].kind).toBe("session_stop");
    expect(hooks[0].payload).toMatchObject({ exit_code: 0 });
  });

  it("finalize emits session_error on non-zero exit and attaches stderr_tail", () => {
    const state = emptyState();
    const { hooks } = finalize(state, { exit_code: 1, signal: undefined, stderr_tail: "boom" });
    expect(hooks).toHaveLength(1);
    expect(hooks[0].kind).toBe("session_error");
    expect(hooks[0].payload).toMatchObject({ exit_code: 1, stderr_tail: "boom" });
  });

  it("finalize flushes any pending partial line", () => {
    const state = emptyState();
    parseChunk(
      `{"type":"item.completed","item":{"id":"i0","type":"command_execution","exit_code":0,"status":"completed"}}`,
      state,
    );
    // No trailing newline; line is still in carry until finalize.
    const { hooks } = finalize(state, { exit_code: 0, signal: undefined, stderr_tail: "" });
    // First the flushed post_tool, then session_stop.
    expect(hooks.map((h) => h.kind)).toEqual(["post_tool", "session_stop"]);
  });
});
```

- [ ] **Step 2: Run and verify failure**

Run: `pnpm -F @cliptown/adapter-codex test`

Expected: FAIL — module `../src/event_parser.js` not found.

---

## Task 2: Codex event_parser — implementation

**Files:**
- Create: `packages/adapters/codex/src/event_parser.ts`

- [ ] **Step 1: Write the parser module**

```ts
// packages/adapters/codex/src/event_parser.ts
import type { HookEvent, HookKind } from "@cliptown/adapter-core";

/**
 * Streaming JSONL parser for codex's `exec --json` stdout. Emits HookEvents
 * directly so the codex adapter can forward them into opts.onHook without
 * going through the (unused-for-codex) HTTP hook bridge.
 *
 * Pure module — no I/O. State is held in CodexParserState so a single
 * parser instance can survive across stdout chunks split mid-line. Tests
 * drive parseChunk + finalize directly with fixture strings.
 */

export interface CodexUsageAccum {
  in_tokens: number;
  out_tokens: number;
  saw: boolean;
}

export interface CodexParserState {
  seq: number;
  usage: CodexUsageAccum;
  /** Partial line buffer carried across parseChunk calls. */
  carry: string;
}

export function emptyState(): CodexParserState {
  return {
    seq: 0,
    usage: { in_tokens: 0, out_tokens: 0, saw: false },
    carry: "",
  };
}

export interface ParseChunkResult {
  hooks: HookEvent[];
}

export function parseChunk(chunk: string, state: CodexParserState): ParseChunkResult {
  const combined = state.carry + chunk;
  const lines = combined.split("\n");
  // Last element is the partial tail (no trailing \n) or "" if chunk ended on \n.
  state.carry = lines.pop() ?? "";
  const hooks: HookEvent[] = [];
  for (const line of lines) {
    parseLine(line, state, hooks);
  }
  return { hooks };
}

export interface FinalizeInfo {
  exit_code: number;
  signal: string | undefined;
  stderr_tail: string;
}

export interface FinalizeResult {
  hooks: HookEvent[];
}

export function finalize(state: CodexParserState, info: FinalizeInfo): FinalizeResult {
  const hooks: HookEvent[] = [];
  // Flush any held-back line that didn't end in \n.
  if (state.carry.length > 0) {
    parseLine(state.carry, state, hooks);
    state.carry = "";
  }
  const kind: HookKind = info.exit_code === 0 ? "session_stop" : "session_error";
  const payload: Record<string, unknown> = { exit_code: info.exit_code };
  if (info.signal) payload.signal = info.signal;
  if (kind === "session_error") payload.stderr_tail = info.stderr_tail;
  hooks.push({
    kind,
    tool: "",
    payload,
    seq: ++state.seq,
    ts_ms: Date.now(),
  });
  return { hooks };
}

function parseLine(line: string, state: CodexParserState, out: HookEvent[]): void {
  const trimmed = line.trim();
  if (trimmed.length === 0) return;
  let evt: { type?: string; item?: { type?: string; tool?: string }; usage?: { input_tokens?: number; cached_input_tokens?: number; output_tokens?: number } };
  try {
    evt = JSON.parse(line);
  } catch {
    return;
  }
  if (evt.type === "item.started" || evt.type === "item.completed") {
    const item = evt.item;
    if (!item) return;
    if (item.type === "command_execution") {
      out.push({
        kind: evt.type === "item.started" ? "pre_tool" : "post_tool",
        tool: "shell",
        payload: item,
        seq: ++state.seq,
        ts_ms: Date.now(),
      });
    } else if (item.type === "mcp_tool_call") {
      out.push({
        kind: evt.type === "item.started" ? "pre_tool" : "post_tool",
        tool: typeof item.tool === "string" && item.tool.length > 0 ? item.tool : "mcp",
        payload: item,
        seq: ++state.seq,
        ts_ms: Date.now(),
      });
    }
    return;
  }
  if (evt.type === "turn.completed" && evt.usage) {
    state.usage.in_tokens +=
      (evt.usage.input_tokens ?? 0) + (evt.usage.cached_input_tokens ?? 0);
    state.usage.out_tokens += evt.usage.output_tokens ?? 0;
    state.usage.saw = true;
  }
}
```

- [ ] **Step 2: Run and verify all event_parser tests pass**

Run: `pnpm -F @cliptown/adapter-codex test`

Expected: PASS — all 9 tests in `event_parser.test.ts` green; existing `hooks.test.ts` still passes.

- [ ] **Step 3: Commit**

```bash
git add packages/adapters/codex/src/event_parser.ts packages/adapters/codex/test/event_parser.test.ts
git commit -m "feat(adapter-codex): streaming JSONL → HookEvent parser

Pure module that converts codex exec --json stdout into HookEvents
(pre_tool/post_tool for command_execution + mcp_tool_call, session_stop/
session_error at exit) and accumulates UsageReport tokens. Used by the
adapter's stdout handler to forward hooks via opts.onHook without the
HTTP bridge (which codex never POSTed to).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Codex index.ts — wire parser, drop bridge, fixture-branch synth

**Files:**
- Modify: `packages/adapters/codex/src/index.ts`
- Modify: `packages/adapters/codex/test/hooks.test.ts`

- [ ] **Step 1: Update hooks.test.ts to assert the new capability shape**

Replace the body of `describe("codexAdapter shape", ...)` with:

```ts
import { describe, it, expect } from "vitest";
import { codexAdapter } from "../src/index.js";

describe("codexAdapter shape", () => {
  it("declares correct id + capabilities", () => {
    expect(codexAdapter.id).toBe("codex");
    expect(codexAdapter.capabilities.block_on_stop).toBe(false);
    expect(codexAdapter.capabilities.inject_context).toBe(false);
    expect(codexAdapter.capabilities.hooks).toEqual([
      "pre_tool", "post_tool", "session_stop", "session_error",
    ]);
    expect(typeof codexAdapter.spawn).toBe("function");
  });
});
```

- [ ] **Step 2: Run to verify the test fails**

Run: `pnpm -F @cliptown/adapter-codex test -- --run hooks.test.ts`

Expected: FAIL — current adapter only advertises `["session_stop","session_error"]`.

- [ ] **Step 3: Rewrite codex index.ts**

Replace the entire contents of `packages/adapters/codex/src/index.ts` with:

```ts
import { spawn as nodeSpawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  type AdapterCapabilities, type BackendAdapter,
  type HookEvent, type SpawnOpts, type SpawnResult, type UsageReport,
} from "@cliptown/adapter-core";
import { emptyState, parseChunk, finalize, type CodexParserState } from "./event_parser.js";

/**
 * Codex CLI adapter. Spawns `codex exec --json` (codex-cli ≥ 0.124) and
 * forwards hook events to opts.onHook by parsing the CLI's JSONL stdout
 * stream in-process. No HTTP hook bridge — codex never POSTed to one;
 * its native event surface is the JSONL stream.
 *
 * Integration choices (M9.10 follow-up — hook bridge):
 *   - MCP via `-c mcp_servers.cliptown.{url,bearer_token_env_var}` overrides;
 *     bearer token read from CLIPTOWN_MCP_TOKEN env (per-spawn).
 *   - `--full-auto` + `--dangerously-bypass-approvals-and-sandbox` for the
 *     hermetic non-interactive run (SMOKE_DIR + per-spawn token gate the
 *     enforcement boundary).
 *   - `--json` makes stdout JSONL: thread.started, turn.started,
 *     item.started/completed (command_execution | mcp_tool_call |
 *     agent_message), turn.completed. event_parser.ts converts these
 *     into HookEvents.
 *   - `model_id` for the budget ladder uses a stable cliptown-owned
 *     identifier (`gpt-5-chatgpt`) by default; override via CODEX_MODEL_ID.
 *
 * Override the binary via `SpawnOpts.bin` or `CLIPTOWN_FIXTURE_CLI` for
 * tests against the fixture-cli shim. In fixture mode the adapter emits
 * a synthetic [pre_tool, post_tool, session_stop] sequence into
 * opts.onHook directly (fixture-cli speaks claude-code's hook protocol,
 * not codex JSONL, so we synthesize here).
 */

const CAPS: AdapterCapabilities = {
  hooks: ["pre_tool", "post_tool", "session_stop", "session_error"],
  inject_context: false,
  block_on_stop: false,
};

const TOKEN_ENV_VAR = "CLIPTOWN_MCP_TOKEN";
const MODEL_ID_ENV = "CODEX_MODEL_ID";
const DEFAULT_MODEL_ID = "gpt-5-chatgpt";

export const codexAdapter: BackendAdapter = {
  id: "codex",
  capabilities: CAPS,
  async spawn(opts: SpawnOpts): Promise<SpawnResult> {
    const onHook = opts.onHook ?? (() => { /* noop */ });
    const modelId = opts.env?.[MODEL_ID_ENV] ?? process.env[MODEL_ID_ENV] ?? DEFAULT_MODEL_ID;

    const cfgDir = await mkdtemp(join(tmpdir(), "ct-codex-"));
    const bin = opts.bin ?? process.env.CLIPTOWN_FIXTURE_CLI ?? "codex";
    const isFixture = !!(opts.bin || process.env.CLIPTOWN_FIXTURE_CLI);
    const env = {
      ...process.env,
      ...opts.env,
      [TOKEN_ENV_VAR]: opts.mcp_token,
    };

    let args: string[];
    if (isFixture) {
      args = ["--prompt", opts.prompt];
    } else {
      args = [
        "exec",
        "--ignore-user-config",
        "--dangerously-bypass-approvals-and-sandbox",
        "--skip-git-repo-check",
        "--ephemeral",
        "--cd", opts.cwd,
        "--color", "never",
        "--json",
        "-c", `mcp_servers.cliptown.url="${opts.mcp_world_url}/mcp"`,
        "-c", `mcp_servers.cliptown.bearer_token_env_var="${TOKEN_ENV_VAR}"`,
        opts.prompt,
      ];
    }

    const child = nodeSpawn(
      bin,
      args,
      { cwd: opts.cwd, env, stdio: ["ignore", "pipe", "pipe"] },
    );

    const parser: CodexParserState = emptyState();
    let stderrTail = "";
    const STDERR_TAIL_MAX = 4096;

    const emit = (e: HookEvent) => {
      try { onHook(e); } catch { /* swallow consumer errors */ }
    };

    child.stdout?.on("data", (b: Buffer) => {
      const s = b.toString("utf-8");
      opts.onLog?.("stdout", s);
      if (isFixture) return; // synthetic hooks fired in fixture branch below
      const { hooks } = parseChunk(s, parser);
      for (const h of hooks) emit(h);
    });
    child.stderr?.on("data", (b: Buffer) => {
      const s = b.toString("utf-8");
      opts.onLog?.("stderr", s);
      stderrTail = (stderrTail + s).slice(-STDERR_TAIL_MAX);
    });

    // Fixture branch synthesizes the hook sequence so contract tests see
    // the new capability surface in action without teaching fixture-cli
    // to speak codex JSONL.
    if (isFixture) {
      const now = () => Date.now();
      let seq = 0;
      emit({ kind: "pre_tool", tool: "shell", payload: { tool: "shell", args: { cmd: "echo hi" } }, seq: ++seq, ts_ms: now() });
      emit({ kind: "post_tool", tool: "shell", payload: { tool: "shell", exit_code: 0 }, seq: ++seq, ts_ms: now() });
      emit({ kind: "session_stop", tool: "", payload: { exit_code: 0, prompt: opts.prompt }, seq: ++seq, ts_ms: now() });
    }

    const exit = new Promise<{ exit_code: number; signal?: string }>((resolve) => {
      child.on("exit", (code, signal) => {
        resolve({ exit_code: code ?? -1, signal: signal ?? undefined });
      });
    });

    const result: SpawnResult = {
      pid: child.pid ?? -1,
      async wait() {
        const r = await exit;
        await rm(cfgDir, { recursive: true, force: true });
        if (isFixture) return { ...r, usage: undefined };
        // Emit terminal hook + accumulate any partial line.
        const { hooks } = finalize(parser, {
          exit_code: r.exit_code,
          signal: r.signal,
          stderr_tail: stderrTail,
        });
        for (const h of hooks) emit(h);
        const usage: UsageReport | undefined = parser.usage.saw
          ? { in_tokens: parser.usage.in_tokens, out_tokens: parser.usage.out_tokens, model_id: modelId }
          : undefined;
        return { ...r, usage };
      },
      kill(signal: NodeJS.Signals = "SIGTERM") {
        try { child.kill(signal); } catch { /* noop */ }
      },
    };

    return result;
  },
};

export type { HookEvent };
```

- [ ] **Step 4: Run all codex tests**

Run: `pnpm -F @cliptown/adapter-codex test`

Expected: PASS — `hooks.test.ts` and `event_parser.test.ts` both green.

- [ ] **Step 5: Run cross-adapter contract test**

Run: `pnpm -F @cliptown/worker test -- --run contract.test.ts`

Expected: PASS — M8.3 case for codex sees the new hooks list; `it.each` test continues to pass because it only checks for `session_stop`/`session_error` (both still present).

- [ ] **Step 6: Commit**

```bash
git add packages/adapters/codex/src/index.ts packages/adapters/codex/test/hooks.test.ts
git commit -m "feat(adapter-codex): wire JSONL parser, drop dead HTTP bridge

Codex stdout (codex exec --json) now drives opts.onHook via event_parser:
- item.started/completed for command_execution → pre_tool/post_tool 'shell'
- item.started/completed for mcp_tool_call → pre_tool/post_tool with
  item.tool as the tool name
- process exit → session_stop (code 0) or session_error (non-zero, with
  stderr_tail)

Removes startHookBridge + CODEX_HOOK_PORT (codex never POSTed to it).
Caps lift to [pre_tool, post_tool, session_stop, session_error]. Fixture
branch synthesizes the same shape so the contract test surface stays
green without teaching fixture-cli to speak codex JSONL.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Opencode event_mapper — failing tests

**Files:**
- Test: `packages/adapters/opencode/test/event_mapper.test.ts`

- [ ] **Step 1: Create the failing test file**

```ts
// packages/adapters/opencode/test/event_mapper.test.ts
import { describe, it, expect } from "vitest";
import { emptyMapState, mapEvent, type MapState } from "../src/event_mapper.js";

function partTool(callID: string, tool: string, status: string, input: unknown = null, output: unknown = null) {
  return {
    type: "message.part.updated" as const,
    properties: {
      part: {
        type: "tool",
        tool,
        callID,
        state: {
          status,
          input,
          output,
          time: { start: 1, end: 2 },
        },
      },
    },
  };
}

describe("opencode event_mapper", () => {
  it("emits pre_tool on first running and post_tool on first completed", () => {
    const state = emptyMapState();
    const events = [
      partTool("c1", "bash", "pending"),
      partTool("c1", "bash", "running", { command: "echo hi" }),
      partTool("c1", "bash", "running", { command: "echo hi" }), // dedup
      partTool("c1", "bash", "running", { command: "echo hi" }),
      partTool("c1", "bash", "completed", { command: "echo hi" }, "hi\n"),
    ];
    const hooks = events.flatMap((e) => mapEvent(e, state).hooks);
    expect(hooks.map((h) => [h.kind, h.tool])).toEqual([
      ["pre_tool", "bash"],
      ["post_tool", "bash"],
    ]);
    expect(hooks[0].payload).toMatchObject({ command: "echo hi" });
    expect(hooks[1].payload).toMatchObject({ output: "hi\n" });
    expect(hooks[0].seq).toBe(1);
    expect(hooks[1].seq).toBe(2);
  });

  it("treats failed as terminal and emits post_tool exactly once", () => {
    const state = emptyMapState();
    const events = [
      partTool("c1", "read", "running"),
      partTool("c1", "read", "failed", null, { error: "ENOENT" }),
      partTool("c1", "read", "failed", null, { error: "ENOENT" }), // dedup
    ];
    const hooks = events.flatMap((e) => mapEvent(e, state).hooks);
    expect(hooks.map((h) => h.kind)).toEqual(["pre_tool", "post_tool"]);
  });

  it("interleaves two concurrent tool calls cleanly", () => {
    const state = emptyMapState();
    const events = [
      partTool("c1", "bash", "running"),
      partTool("c2", "read", "running"),
      partTool("c1", "bash", "completed"),
      partTool("c2", "read", "completed"),
    ];
    const hooks = events.flatMap((e) => mapEvent(e, state).hooks);
    expect(hooks.map((h) => [h.kind, h.tool])).toEqual([
      ["pre_tool", "bash"],
      ["pre_tool", "read"],
      ["post_tool", "bash"],
      ["post_tool", "read"],
    ]);
  });

  it("emits session_stop on session.idle", () => {
    const state = emptyMapState();
    const { hooks } = mapEvent({ type: "session.idle", properties: {} }, state);
    expect(hooks).toHaveLength(1);
    expect(hooks[0].kind).toBe("session_stop");
    expect(hooks[0].tool).toBe("");
  });

  it("accumulates usage from step-finish parts", () => {
    const state = emptyMapState();
    const sf = (input: number, output: number, cost: number) => ({
      type: "message.part.updated" as const,
      properties: {
        part: {
          type: "step-finish",
          tokens: { input, output, reasoning: 0, cache: { write: 0, read: 0 } },
          cost,
        },
      },
    });
    mapEvent(sf(100, 20, 0.01), state);
    mapEvent(sf(150, 30, 0.02), state);
    expect(state.usage).toEqual({ in_tokens: 250, out_tokens: 50, cost_usd: 0.03, saw: true });
  });

  it("ignores irrelevant event types without crashing", () => {
    const state = emptyMapState();
    for (const t of [
      "server.connected", "server.heartbeat", "session.created",
      "session.updated", "session.diff", "message.updated", "message.part.delta",
    ]) {
      const { hooks } = mapEvent({ type: t, properties: {} }, state);
      expect(hooks).toEqual([]);
    }
  });

  it("ignores tool parts in pending state", () => {
    const state = emptyMapState();
    const { hooks } = mapEvent(partTool("c1", "bash", "pending"), state);
    expect(hooks).toEqual([]);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm -F @cliptown/adapter-opencode test -- --run event_mapper.test.ts`

Expected: FAIL — module `../src/event_mapper.js` not found.

---

## Task 5: Opencode event_mapper — implementation

**Files:**
- Create: `packages/adapters/opencode/src/event_mapper.ts`

- [ ] **Step 1: Write the mapper module**

```ts
// packages/adapters/opencode/src/event_mapper.ts
import type { HookEvent } from "@cliptown/adapter-core";

/**
 * Maps opencode `/event` SSE events to HookEvents. The interesting
 * surface is `message.part.updated` with `part.type === "tool"`, which
 * fires multiple times per call (pending → running → ... → completed).
 * We dedup by callID and emit exactly one pre_tool (on first running)
 * and one post_tool (on first terminal status). step-finish parts feed
 * the UsageReport accumulator.
 *
 * Pure module — no I/O. State lives in MapState so a single mapper
 * instance can span an opencode session.
 */

export interface MapUsageAccum {
  in_tokens: number;
  out_tokens: number;
  cost_usd: number;
  saw: boolean;
}

interface CallTracker {
  fired_pre: boolean;
  fired_post: boolean;
}

export interface MapState {
  seq: number;
  usage: MapUsageAccum;
  calls: Map<string, CallTracker>;
}

export function emptyMapState(): MapState {
  return {
    seq: 0,
    usage: { in_tokens: 0, out_tokens: 0, cost_usd: 0, saw: false },
    calls: new Map(),
  };
}

export interface SseEvent {
  type?: string;
  properties?: {
    part?: {
      type?: string;
      tool?: string;
      callID?: string;
      state?: {
        status?: string;
        input?: unknown;
        output?: unknown;
        time?: { start?: number; end?: number };
      };
      tokens?: {
        input?: number;
        output?: number;
        reasoning?: number;
        cache?: { write?: number; read?: number };
      };
      cost?: number;
    };
  };
}

export interface MapResult {
  hooks: HookEvent[];
}

export function mapEvent(evt: SseEvent, state: MapState): MapResult {
  const hooks: HookEvent[] = [];
  const t = evt.type;

  if (t === "session.idle") {
    hooks.push({
      kind: "session_stop",
      tool: "",
      payload: {},
      seq: ++state.seq,
      ts_ms: Date.now(),
    });
    return { hooks };
  }

  if (t === "session.status") {
    // Spec note: exact error variant shape unconfirmed. If a future probe
    // shows a terminal-error variant carrying e.g. {phase:"errored"} or
    // {error}, add it here. Until then this branch is dormant.
    return { hooks };
  }

  if (t !== "message.part.updated") return { hooks };
  const part = evt.properties?.part;
  if (!part) return { hooks };

  if (part.type === "step-finish") {
    const tokens = part.tokens;
    if (tokens) {
      state.usage.in_tokens +=
        (tokens.input ?? 0) +
        (tokens.cache?.write ?? 0) +
        (tokens.cache?.read ?? 0);
      state.usage.out_tokens += (tokens.output ?? 0) + (tokens.reasoning ?? 0);
      state.usage.cost_usd += part.cost ?? 0;
      state.usage.saw = true;
    }
    return { hooks };
  }

  if (part.type !== "tool") return { hooks };

  const callID = part.callID;
  if (!callID) return { hooks };
  const status = part.state?.status ?? "";
  const tool = part.tool ?? "";

  let tracker = state.calls.get(callID);
  if (!tracker) {
    tracker = { fired_pre: false, fired_post: false };
    state.calls.set(callID, tracker);
  }

  if (status === "running" && !tracker.fired_pre) {
    tracker.fired_pre = true;
    hooks.push({
      kind: "pre_tool",
      tool,
      payload: part.state?.input ?? null,
      seq: ++state.seq,
      ts_ms: Date.now(),
    });
    return { hooks };
  }

  if ((status === "completed" || status === "failed") && !tracker.fired_post) {
    // If the model emits completed without ever passing through running,
    // synthesize a pre first so consumers always see pre before post.
    if (!tracker.fired_pre) {
      tracker.fired_pre = true;
      hooks.push({
        kind: "pre_tool",
        tool,
        payload: part.state?.input ?? null,
        seq: ++state.seq,
        ts_ms: Date.now(),
      });
    }
    tracker.fired_post = true;
    hooks.push({
      kind: "post_tool",
      tool,
      payload: {
        output: part.state?.output ?? null,
        status,
        time: part.state?.time,
      },
      seq: ++state.seq,
      ts_ms: Date.now(),
    });
    return { hooks };
  }

  return { hooks };
}
```

- [ ] **Step 2: Run and verify mapper tests pass**

Run: `pnpm -F @cliptown/adapter-opencode test -- --run event_mapper.test.ts`

Expected: PASS — all 7 tests green.

- [ ] **Step 3: Commit**

```bash
git add packages/adapters/opencode/src/event_mapper.ts packages/adapters/opencode/test/event_mapper.test.ts
git commit -m "feat(adapter-opencode): SSE event → HookEvent mapper

Pure module that converts opencode /event SSE frames into HookEvents.
Dedups per callID so pre_tool fires once (on first 'running' transition)
and post_tool fires once (on first 'completed'/'failed'). step-finish
parts feed the UsageReport accumulator (tokens + cost). session.idle
maps to session_stop.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Opencode sse_client — failing tests

**Files:**
- Test: `packages/adapters/opencode/test/sse_client.test.ts`

- [ ] **Step 1: Create the failing test file**

```ts
// packages/adapters/opencode/test/sse_client.test.ts
import { describe, it, expect } from "vitest";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { subscribeSse } from "../src/sse_client.js";

interface ServerHandle {
  server: Server;
  port: number;
  push: (frame: string) => void;
  end: () => void;
  close: () => Promise<void>;
}

async function startServer(): Promise<ServerHandle> {
  let writer: { write: (s: string) => void; end: () => void } | null = null;
  const server = createServer((req, res) => {
    if (req.url !== "/event") {
      res.writeHead(404);
      res.end();
      return;
    }
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    writer = {
      write: (s) => res.write(s),
      end: () => res.end(),
    };
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve()));
  const port = (server.address() as AddressInfo).port;
  return {
    server,
    port,
    push: (frame) => { writer?.write(frame); },
    end: () => { writer?.end(); },
    async close() {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

describe("sse_client", () => {
  it("yields parsed JSON events from data: frames", async () => {
    const h = await startServer();
    const ctrl = new AbortController();
    const got: unknown[] = [];
    const sub = (async () => {
      for await (const evt of subscribeSse(`http://127.0.0.1:${h.port}/event`, ctrl.signal)) {
        got.push(evt);
        if (got.length === 2) ctrl.abort();
      }
    })();
    // Push two frames, then close.
    await new Promise((r) => setTimeout(r, 50));
    h.push(`data: {"type":"server.connected"}\n\n`);
    h.push(`data: {"type":"session.idle"}\n\n`);
    await sub;
    expect(got).toEqual([
      { type: "server.connected" },
      { type: "session.idle" },
    ]);
    await h.close();
  });

  it("handles a frame split across two TCP chunks", async () => {
    const h = await startServer();
    const ctrl = new AbortController();
    const got: unknown[] = [];
    const sub = (async () => {
      for await (const evt of subscribeSse(`http://127.0.0.1:${h.port}/event`, ctrl.signal)) {
        got.push(evt);
        if (got.length === 1) ctrl.abort();
      }
    })();
    await new Promise((r) => setTimeout(r, 50));
    h.push(`data: {"type":"sess`);
    await new Promise((r) => setTimeout(r, 30));
    h.push(`ion.idle"}\n\n`);
    await sub;
    expect(got).toEqual([{ type: "session.idle" }]);
    await h.close();
  });

  it("skips malformed JSON without crashing", async () => {
    const h = await startServer();
    const ctrl = new AbortController();
    const got: unknown[] = [];
    const sub = (async () => {
      for await (const evt of subscribeSse(`http://127.0.0.1:${h.port}/event`, ctrl.signal)) {
        got.push(evt);
        if (got.length === 1) ctrl.abort();
      }
    })();
    await new Promise((r) => setTimeout(r, 50));
    h.push(`data: not json\n\n`);
    h.push(`data: {"type":"session.idle"}\n\n`);
    await sub;
    expect(got).toEqual([{ type: "session.idle" }]);
    await h.close();
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm -F @cliptown/adapter-opencode test -- --run sse_client.test.ts`

Expected: FAIL — module `../src/sse_client.js` not found.

---

## Task 7: Opencode sse_client — implementation

**Files:**
- Create: `packages/adapters/opencode/src/sse_client.ts`

- [ ] **Step 1: Write the SSE client**

```ts
// packages/adapters/opencode/src/sse_client.ts
/**
 * Line-buffered SSE consumer over Node's built-in fetch + ReadableStream.
 * Yields one parsed JSON object per `data: ` event. Frames are delimited
 * by a blank line (`\n\n`). Aborting the AbortSignal closes the underlying
 * connection and ends iteration.
 *
 * Scope: opencode /event SSE only. We don't implement reconnection or
 * `id:`/`event:` handling (opencode doesn't use them for our purposes).
 */

export async function* subscribeSse(
  url: string,
  signal: AbortSignal,
): AsyncGenerator<unknown, void, void> {
  const res = await fetch(url, { signal });
  if (!res.ok || !res.body) {
    throw new Error(`sse subscribe failed: status=${res.status}`);
  }
  const reader = res.body.getReader();
  const decoder = new TextDecoder("utf-8");
  let buf = "";
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      // Process complete frames (delimited by \n\n) in order.
      let idx: number;
      while ((idx = buf.indexOf("\n\n")) !== -1) {
        const frame = buf.slice(0, idx);
        buf = buf.slice(idx + 2);
        const data = extractDataPayload(frame);
        if (data === undefined) continue;
        try {
          yield JSON.parse(data);
        } catch {
          // skip malformed payload
        }
      }
    }
  } finally {
    try { reader.cancel(); } catch { /* noop */ }
  }
}

function extractDataPayload(frame: string): string | undefined {
  // A frame may carry multiple "data:" lines; concatenate them with \n
  // per the SSE spec. Other prefixes (id:, event:) are ignored here.
  const parts: string[] = [];
  for (const line of frame.split("\n")) {
    if (line.startsWith("data:")) {
      parts.push(line.slice(5).replace(/^ /, ""));
    }
  }
  if (parts.length === 0) return undefined;
  return parts.join("\n");
}
```

- [ ] **Step 2: Run and verify all sse_client tests pass**

Run: `pnpm -F @cliptown/adapter-opencode test -- --run sse_client.test.ts`

Expected: PASS — all 3 tests green.

- [ ] **Step 3: Commit**

```bash
git add packages/adapters/opencode/src/sse_client.ts packages/adapters/opencode/test/sse_client.test.ts
git commit -m "feat(adapter-opencode): line-buffered SSE consumer

Node fetch + ReadableStream-based async generator that yields one parsed
JSON object per SSE 'data:' frame. Handles frames split across TCP
chunks; skips malformed payloads; aborts cleanly on signal. Used by the
opencode adapter to subscribe to opencode serve's /event stream.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Opencode serve_lifecycle — implementation

**Files:**
- Create: `packages/adapters/opencode/src/serve_lifecycle.ts`

(No separate unit test; covered by smoke + by the index.ts integration.)

- [ ] **Step 1: Write the lifecycle module**

```ts
// packages/adapters/opencode/src/serve_lifecycle.ts
import { spawn as nodeSpawn, type ChildProcess } from "node:child_process";

/**
 * Spawns `opencode serve --port 0 --pure --hostname 127.0.0.1` and waits
 * for its "listening on http://127.0.0.1:<port>" log line on stderr to
 * capture the chosen port. Returns a handle the adapter uses to read
 * the URL, await the process exit, and terminate it.
 *
 * Why this lives in its own module:
 *   - The "wait for listening line" dance is the most fragile part of
 *     the opencode adapter; isolating it makes the index.ts orchestration
 *     readable and the lifecycle independently swappable if opencode
 *     adds a /ready endpoint or changes its log format.
 *   - Keeping the child process reference inside lets the adapter call
 *     `kill()` without keeping a `child` variable around in index.ts.
 */

export interface ServeHandle {
  /** Base URL like `http://127.0.0.1:54321` (no trailing slash). */
  url: string;
  /** Resolves when the server process exits. */
  exit: Promise<{ exit_code: number; signal?: string }>;
  /** SIGTERM the server child. Idempotent. */
  kill(signal?: NodeJS.Signals): void;
  /** The underlying child for log forwarding. */
  child: ChildProcess;
}

export interface StartServeOpts {
  bin: string;
  /** Working dir for the child (does not affect listening port). */
  cwd: string;
  /** Extra env merged onto inherited env. */
  env?: NodeJS.ProcessEnv;
  /** Max ms to wait for the listening line. Default 15000. */
  readyTimeoutMs?: number;
  /** Forwarded so callers can tee stderr to operator logs. */
  onLog?: (stream: "stdout" | "stderr", line: string) => void;
}

const LISTENING_RE = /opencode server listening on (http:\/\/[^\s]+)/;

export async function startServe(opts: StartServeOpts): Promise<ServeHandle> {
  const readyMs = opts.readyTimeoutMs ?? 15000;
  const env = { ...process.env, ...opts.env };
  const child = nodeSpawn(
    opts.bin,
    ["serve", "--port", "0", "--pure", "--hostname", "127.0.0.1", "--print-logs"],
    { cwd: opts.cwd, env, stdio: ["ignore", "pipe", "pipe"] },
  );

  const exit = new Promise<{ exit_code: number; signal?: string }>((resolve) => {
    child.on("exit", (code, signal) => {
      resolve({ exit_code: code ?? -1, signal: signal ?? undefined });
    });
  });

  const url = await new Promise<string>((resolve, reject) => {
    let stderrBuf = "";
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      reject(new Error(`opencode serve did not announce listening URL within ${readyMs}ms`));
    }, readyMs);

    const onChunk = (b: Buffer) => {
      const s = b.toString("utf-8");
      opts.onLog?.("stderr", s);
      stderrBuf += s;
      const m = LISTENING_RE.exec(stderrBuf);
      if (m && !settled) {
        settled = true;
        clearTimeout(timer);
        resolve(m[1].replace(/\/$/, ""));
      }
    };
    child.stderr?.on("data", onChunk);
    child.stdout?.on("data", (b: Buffer) => opts.onLog?.("stdout", b.toString("utf-8")));
    child.on("exit", (code) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      reject(new Error(`opencode serve exited before announcing listening URL (code=${code})`));
    });
  });

  return {
    url,
    exit,
    kill(signal: NodeJS.Signals = "SIGTERM") {
      try { child.kill(signal); } catch { /* noop */ }
    },
    child,
  };
}
```

- [ ] **Step 2: Commit (no test addition this step)**

```bash
git add packages/adapters/opencode/src/serve_lifecycle.ts
git commit -m "feat(adapter-opencode): opencode serve lifecycle helper

Spawns opencode serve --port 0 --pure --hostname 127.0.0.1 and parses
the 'listening on <url>' stderr line to capture the random port. Exposes
url, exit promise, kill(). Coverage via index.ts integration + § 11.9
smoke; no separate unit test (the value is in the actual spawn dance,
which is integration-shaped).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Opencode session_client — implementation

**Files:**
- Create: `packages/adapters/opencode/src/session_client.ts`

- [ ] **Step 1: Write the session client**

```ts
// packages/adapters/opencode/src/session_client.ts
/**
 * Thin fetch wrappers around opencode serve's session API. Kept tiny and
 * dependency-free so it's trivial to swap or stub. opencode runs on
 * 127.0.0.1 with no auth (same trust model as our other 127.0.0.1
 * helpers).
 */

export interface OpencodeModel {
  providerID: string;
  modelID: string;
}

export interface CreatedSession {
  id: string;
}

export async function createSession(baseUrl: string, cwd: string): Promise<CreatedSession> {
  const res = await fetch(`${baseUrl}/session`, {
    method: "POST",
    headers: { "x-opencode-directory": cwd },
  });
  if (!res.ok) {
    throw new Error(`createSession failed: status=${res.status}`);
  }
  const body = (await res.json()) as { id: string };
  return { id: body.id };
}

export interface SendMessageOpts {
  sessionId: string;
  prompt: string;
  agent: string;
  model: OpencodeModel;
}

export async function sendMessage(baseUrl: string, opts: SendMessageOpts): Promise<void> {
  const res = await fetch(`${baseUrl}/session/${opts.sessionId}/message`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      parts: [{ type: "text", text: opts.prompt }],
      agent: opts.agent,
      model: opts.model,
    }),
  });
  if (!res.ok) {
    throw new Error(`sendMessage failed: status=${res.status}`);
  }
  // Drain the response body so the connection releases.
  await res.text();
}

export async function deleteSession(baseUrl: string, sessionId: string): Promise<void> {
  await fetch(`${baseUrl}/session/${sessionId}`, { method: "DELETE" });
}
```

- [ ] **Step 2: Commit (no test — covered by integration + smoke)**

```bash
git add packages/adapters/opencode/src/session_client.ts
git commit -m "feat(adapter-opencode): session API fetch wrappers

POST /session, POST /session/<id>/message, DELETE /session/<id>. Dep-free
wrappers used by the adapter's serve+REST+SSE flow. No unit test;
covered by index.ts integration and § 11.9 smoke.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Opencode index.ts — rewrite around serve + REST + SSE

**Files:**
- Modify: `packages/adapters/opencode/src/index.ts`
- Modify: `packages/adapters/opencode/test/hooks.test.ts`

- [ ] **Step 1: Update hooks.test.ts to assert the new capability shape**

Replace the body of `packages/adapters/opencode/test/hooks.test.ts` with:

```ts
import { describe, it, expect } from "vitest";
import { opencodeAdapter } from "../src/index.js";

describe("opencodeAdapter shape", () => {
  it("declares correct id + capabilities", () => {
    expect(opencodeAdapter.id).toBe("opencode");
    expect(opencodeAdapter.capabilities.inject_context).toBe(true);
    expect(opencodeAdapter.capabilities.block_on_stop).toBe(false);
    expect(opencodeAdapter.capabilities.hooks).toEqual([
      "pre_tool", "post_tool", "session_stop", "session_error",
    ]);
    expect(typeof opencodeAdapter.spawn).toBe("function");
  });
});
```

- [ ] **Step 2: Run and verify it fails**

Run: `pnpm -F @cliptown/adapter-opencode test -- --run hooks.test.ts`

Expected: FAIL — current adapter advertises `["session_stop","session_error"]`.

- [ ] **Step 3: Rewrite index.ts**

Replace the entire contents of `packages/adapters/opencode/src/index.ts` with:

```ts
import { spawn as nodeSpawn } from "node:child_process";
import { writeFile, rm } from "node:fs/promises";
import { join } from "node:path";
import {
  type AdapterCapabilities, type BackendAdapter,
  type HookEvent, type SpawnOpts, type SpawnResult, type UsageReport,
} from "@cliptown/adapter-core";
import { startServe } from "./serve_lifecycle.js";
import { subscribeSse } from "./sse_client.js";
import { emptyMapState, mapEvent, type MapState } from "./event_mapper.js";
import { createSession, sendMessage, deleteSession } from "./session_client.js";

/**
 * opencode CLI adapter. Drives opencode 1.4.x via its headless server
 * mode (`opencode serve --port 0 --pure`) + REST/SSE so we can observe
 * tool state transitions (pending → running → completed) for true
 * pre_tool/post_tool semantics. The `opencode run --format json` path
 * was abandoned because it only emits already-completed tool_use frames.
 *
 * Integration choices (M9.10 follow-up — hook bridge):
 *   - opencode.json is written to opts.cwd with the cliptown MCP server
 *     entry + model (unchanged from prior version).
 *   - opencode serve runs unsecured on 127.0.0.1 + random port (same
 *     trust model as the existing claude-code hook bridge).
 *   - Subscribe to GET /event SSE; map message.part.updated frames to
 *     HookEvents via event_mapper. session.idle is the terminal signal.
 *   - Tokens + cost accumulated from step-finish parts feed UsageReport
 *     directly (opencode reports USD).
 *
 * Override the binary via `SpawnOpts.bin` or `CLIPTOWN_FIXTURE_CLI`. In
 * fixture mode the adapter emits a synthetic [pre_tool, post_tool,
 * session_stop] sequence into opts.onHook directly (fixture-cli speaks
 * claude-code's settings.json hook protocol, not opencode SSE).
 */

const CAPS: AdapterCapabilities = {
  hooks: ["pre_tool", "post_tool", "session_stop", "session_error"],
  inject_context: true,
  block_on_stop: false,
};

const MODEL_ENV = "OPENCODE_MODEL";
const DEFAULT_MODEL = "openai/gpt-5.4-mini";

function opencodeConfig(worldUrl: string, token: string, model: string): object {
  return {
    $schema: "https://opencode.ai/config.json",
    model,
    mcp: {
      cliptown: {
        type: "remote",
        url: `${worldUrl}/mcp`,
        enabled: true,
        headers: { Authorization: `Bearer ${token}` },
      },
    },
  };
}

async function writeOpencodeConfig(
  cwd: string,
  worldUrl: string,
  token: string,
  model: string,
): Promise<{ cleanup: () => Promise<void> }> {
  const cfgPath = join(cwd, "opencode.json");
  await writeFile(
    cfgPath,
    JSON.stringify(opencodeConfig(worldUrl, token, model), null, 2),
  );
  return {
    async cleanup() {
      await rm(cfgPath, { force: true });
    },
  };
}

function splitProviderModel(spec: string): { providerID: string; modelID: string } {
  const ix = spec.indexOf("/");
  if (ix < 0) return { providerID: "openai", modelID: spec };
  return { providerID: spec.slice(0, ix), modelID: spec.slice(ix + 1) };
}

export const opencodeAdapter: BackendAdapter = {
  id: "opencode",
  capabilities: CAPS,
  async spawn(opts: SpawnOpts): Promise<SpawnResult> {
    const onHook = opts.onHook ?? (() => { /* noop */ });
    const modelSpec = opts.env?.[MODEL_ENV] ?? process.env[MODEL_ENV] ?? DEFAULT_MODEL;

    const bin = opts.bin ?? process.env.CLIPTOWN_FIXTURE_CLI ?? "opencode";
    const isFixture = !!(opts.bin || process.env.CLIPTOWN_FIXTURE_CLI);

    const emit = (e: HookEvent) => {
      try { onHook(e); } catch { /* swallow consumer errors */ }
    };

    // ---- Fixture path ------------------------------------------------
    // fixture-cli speaks claude-code's settings.json protocol, not
    // opencode SSE. We still spawn it (so contract tests get a real
    // child + exit code), but emit a synthetic hook sequence so the
    // capability surface is exercised.
    if (isFixture) {
      const env = { ...process.env, ...opts.env };
      const child = nodeSpawn(
        bin,
        ["--prompt", opts.prompt],
        { cwd: opts.cwd, env, stdio: ["ignore", "pipe", "pipe"] },
      );
      if (opts.onLog) {
        child.stdout?.on("data", (b: Buffer) => opts.onLog?.("stdout", b.toString("utf-8")));
        child.stderr?.on("data", (b: Buffer) => opts.onLog?.("stderr", b.toString("utf-8")));
      }
      const exit = new Promise<{ exit_code: number; signal?: string }>((resolve) => {
        child.on("exit", (code, signal) => {
          resolve({ exit_code: code ?? -1, signal: signal ?? undefined });
        });
      });
      let seq = 0;
      const now = () => Date.now();
      emit({ kind: "pre_tool", tool: "bash", payload: { command: "echo hi" }, seq: ++seq, ts_ms: now() });
      emit({ kind: "post_tool", tool: "bash", payload: { output: "hi\n", status: "completed" }, seq: ++seq, ts_ms: now() });
      emit({ kind: "session_stop", tool: "", payload: {}, seq: ++seq, ts_ms: now() });
      return {
        pid: child.pid ?? -1,
        async wait() {
          const r = await exit;
          return { ...r, usage: undefined };
        },
        kill(signal: NodeJS.Signals = "SIGTERM") {
          try { child.kill(signal); } catch { /* noop */ }
        },
      };
    }

    // ---- Real path: opencode.json + opencode serve + REST + SSE ------
    const cfg = await writeOpencodeConfig(opts.cwd, opts.mcp_world_url, opts.mcp_token, modelSpec);

    const env = {
      ...process.env,
      ...opts.env,
      [MODEL_ENV]: modelSpec,
    };

    let serve: Awaited<ReturnType<typeof startServe>> | null = null;
    try {
      serve = await startServe({ bin, cwd: opts.cwd, env, onLog: opts.onLog });
    } catch (e) {
      await cfg.cleanup();
      throw e;
    }

    const mapState: MapState = emptyMapState();
    const sseCtrl = new AbortController();

    // session_stop arrives via SSE; the SSE consumer task resolves when
    // it sees session.idle, so the adapter knows when the session is
    // actually done (independent of the server staying alive).
    let resolveIdle: () => void;
    const idle = new Promise<void>((resolve) => { resolveIdle = resolve; });

    const sseTask = (async () => {
      try {
        for await (const evt of subscribeSse(`${serve.url}/event`, sseCtrl.signal)) {
          const { hooks } = mapEvent(evt as Record<string, unknown>, mapState);
          for (const h of hooks) {
            emit(h);
            if (h.kind === "session_stop") resolveIdle();
          }
        }
      } catch (e) {
        // AbortError on teardown is expected.
        if ((e as Error).name !== "AbortError") {
          // surface unexpected SSE errors as session_error
          emit({
            kind: "session_error",
            tool: "",
            payload: { reason: "sse_error", message: (e as Error).message },
            seq: 0,
            ts_ms: Date.now(),
          });
          resolveIdle();
        }
      }
    })();

    let sessionId: string | null = null;
    try {
      const created = await createSession(serve.url, opts.cwd);
      sessionId = created.id;
      await sendMessage(serve.url, {
        sessionId,
        prompt: opts.prompt,
        agent: "build",
        model: splitProviderModel(modelSpec),
      });
    } catch (e) {
      // Best-effort teardown then rethrow.
      sseCtrl.abort();
      serve.kill();
      await cfg.cleanup();
      throw e;
    }

    return {
      pid: serve.child.pid ?? -1,
      async wait() {
        // Wait for terminal SSE signal (session.idle → session_stop).
        await idle;
        if (sessionId) {
          try { await deleteSession(serve!.url, sessionId); } catch { /* noop */ }
        }
        sseCtrl.abort();
        await sseTask;
        serve!.kill();
        const serveExit = await serve!.exit;
        await cfg.cleanup();
        const usage: UsageReport | undefined = mapState.usage.saw
          ? {
              in_tokens: mapState.usage.in_tokens,
              out_tokens: mapState.usage.out_tokens,
              cost_usd: mapState.usage.cost_usd,
              model_id: modelSpec,
            }
          : undefined;
        return { exit_code: serveExit.exit_code, signal: serveExit.signal, usage };
      },
      kill(signal: NodeJS.Signals = "SIGTERM") {
        sseCtrl.abort();
        serve!.kill(signal);
      },
    };
  },
};

export type { HookEvent };
```

- [ ] **Step 4: Run all opencode tests**

Run: `pnpm -F @cliptown/adapter-opencode test`

Expected: PASS — `hooks.test.ts`, `event_mapper.test.ts`, `sse_client.test.ts` all green.

- [ ] **Step 5: Run worker contract test (cross-adapter)**

Run: `pnpm -F @cliptown/worker test -- --run contract.test.ts`

Expected: PASS — M8.3 case for opencode picks up new hooks list; the `it.each` test still passes (only asserts `session_stop`/`session_error` presence, both still there).

- [ ] **Step 6: Commit**

```bash
git add packages/adapters/opencode/src/index.ts packages/adapters/opencode/test/hooks.test.ts
git commit -m "feat(adapter-opencode): rebuild around opencode serve + SSE

Replaces the opencode run --format json invocation entirely. The adapter
now:
- writes opencode.json (unchanged)
- spawns opencode serve --port 0 --pure on 127.0.0.1 random port
- subscribes to /event SSE via sse_client
- POST /session + POST /session/<id>/message via session_client
- maps message.part.updated frames to HookEvents via event_mapper
  (true pre_tool/post_tool on tool state transitions, not synthesized
  from a completed-only stream)
- terminates on session.idle, DELETE /session, SIGTERM server

Removes startHookBridge + OPENCODE_HOOK_PORT (opencode never POSTed to
it). Caps lift to [pre_tool, post_tool, session_stop, session_error].
Fixture branch synthesizes the same shape so contract tests stay green.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: Full workspace test sweep

**Files:** none (verification)

- [ ] **Step 1: Run every test suite that could be affected**

Run, in this order:

```bash
pnpm -F @cliptown/adapter-core test
pnpm -F @cliptown/adapter-claude-code test
pnpm -F @cliptown/adapter-codex test
pnpm -F @cliptown/adapter-opencode test
pnpm -F @cliptown/worker test
cargo test -p cliptown-world
```

Expected:
- `adapter-core`: existing tests pass, no new tests added there.
- `adapter-claude-code`: existing tests pass (claude-code path untouched).
- `adapter-codex`: `hooks.test.ts` + new `event_parser.test.ts` green.
- `adapter-opencode`: `hooks.test.ts` + new `event_mapper.test.ts` + new `sse_client.test.ts` green.
- `worker`: all tests including `contract.test.ts` green; count should be the same as before this change (no worker code touched).
- `cargo test -p cliptown-world`: all green; count unchanged (no Rust touched).

If any suite fails, fix the failing test or implementation before proceeding. Do not commit broken state.

- [ ] **Step 2: Run frontend Playwright tests**

Run: `pnpm -F @cliptown/frontend test`

Expected: 14 Playwright tests green (no frontend changes; sanity check).

---

## Task 12: § 11.9 real-LLM smoke verification (manual)

**Files:** none — operator verification step.

This is the integration gate. The smoke costs real money on real APIs; run it once per backend after all unit/contract tests are green.

- [ ] **Step 1: Run smoke for claude_code (must still work — regression check)**

Run:

```bash
CLIPTOWN_BACKEND=claude_code bash scripts/smoke-real-llm.sh
```

Expected:
- exit 0
- `workspaces/<sid>/artifacts/<tid>.md` produced
- task transitions to `awaiting_review`
- worker log contains lines like `[worker] hook: pre_tool tool=...`, `[worker] hook: post_tool tool=...`, `[worker] hook: session_stop tool=` (same as before — claude-code path unchanged)
- `budget_spent_usd` non-zero (claude-code reports cost directly)

- [ ] **Step 2: Run smoke for codex**

Run:

```bash
CLIPTOWN_BACKEND=codex bash scripts/smoke-real-llm.sh
```

Expected:
- exit 0
- artifact produced; task → `awaiting_review`
- worker log NOW contains `[worker] hook: pre_tool tool=shell` and `[worker] hook: post_tool tool=shell` (new — previously empty for codex)
- worker log contains at least one `[worker] hook: pre_tool tool=mcp__cliptown__*` and matching `post_tool` (MCP tool calls now visible)
- worker log contains `[worker] hook: session_stop tool=` at the end
- `budget_spent_usd ≈ $0.10–$0.20` (codex via table; depends on prompt)

If `pre_tool tool=mcp` (the fallback) appears instead of a real tool name, the codex `mcp_tool_call` item's tool field key differs from our assumption (`item.tool`). Capture one such JSONL line from the smoke run, update `event_parser.ts`'s mcp branch + add a fixture case in `event_parser.test.ts`, commit as a follow-up fix.

- [ ] **Step 3: Run smoke for opencode**

Run:

```bash
CLIPTOWN_BACKEND=opencode bash scripts/smoke-real-llm.sh
```

Expected:
- exit 0
- artifact produced; task → `awaiting_review`
- worker log contains `[worker] hook: pre_tool tool=bash` (or `read`, `write`, etc.) and matching `post_tool` lines for every tool the agent uses
- worker log contains `[worker] hook: session_stop tool=` at the end
- `budget_spent_usd` = $0 for gpt-5.4-mini plan, or matches opencode-reported cost

If session_stop never fires (smoke hangs), the `session.idle` event may have a different shape or path in opencode 1.4.3. Watch stderr of the opencode server child (via `opts.onLog`) for hints, capture a sample SSE frame, update `event_mapper.ts` + add a fixture case, commit as a follow-up fix.

- [ ] **Step 4: Final commit if any fixes were needed in Steps 2-3**

If you discovered a real-vs-fixture shape divergence and pushed a fix commit, no further action. Otherwise, no commit needed.

---

## Definition of done

- All test suites green: `cargo test -p cliptown-world`, `pnpm -F @cliptown/worker test`, `pnpm -F @cliptown/adapter-{core,claude-code,codex,opencode} test`, `pnpm -F @cliptown/frontend test`. Counts unchanged from the prior baseline.
- New tests: `event_parser.test.ts` (≥9), `event_mapper.test.ts` (≥7), `sse_client.test.ts` (≥3).
- Capabilities advertised by each adapter match reality:
  - claude_code: `[pre_tool, post_tool, session_stop, session_error]` (unchanged)
  - codex: `[pre_tool, post_tool, session_stop, session_error]` (changed)
  - opencode: `[pre_tool, post_tool, session_stop, session_error]` (changed)
- § 11.9 smoke produces non-empty `pre_tool`/`post_tool` worker-log lines for all three backends.
- Dead `startHookBridge` calls + `*_HOOK_PORT` env vars removed from codex/opencode.
- `adapter-core/hook_bridge.ts` and claude-code's wiring unchanged.
