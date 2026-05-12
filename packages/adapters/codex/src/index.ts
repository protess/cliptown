import { spawn as nodeSpawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  type AdapterCapabilities, type BackendAdapter,
  type HookEvent, type SpawnOpts, type SpawnResult, type UsageReport,
} from "@cliptown/adapter-core";
import { emptyState, parseChunk, finalize, toUsageReport, type CodexParserState } from "./event_parser.js";

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
        const usage: UsageReport | undefined = toUsageReport(parser, modelId);
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
