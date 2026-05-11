import { spawn as nodeSpawn } from "node:child_process";
import { writeFile, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  startHookBridge,
  type AdapterCapabilities, type BackendAdapter, type HookBridge,
  type HookEvent, type SpawnOpts, type SpawnResult, type UsageReport,
} from "@cliptown/adapter-core";

/**
 * Claude Code adapter. Spawns the `claude` CLI (or override) configured to:
 *   - call MCP tools by POSTing to the world's `/mcp` HTTP endpoint
 *     (M9.10 A1' — MCP-at-the-world)
 *   - POST hook events to a per-spawn HTTP bridge
 * The bridge normalizes hook payloads to `HookEvent` and forwards them via
 * `opts.onHook`.
 *
 * Override the binary via `SpawnOpts.bin` or the `CLIPTOWN_FIXTURE_CLI` env var
 * (used by M3.3 contract tests against a stand-in CLI).
 */

const CAPS: AdapterCapabilities = {
  hooks: ["pre_tool", "post_tool", "session_stop", "session_error"],
  inject_context: true,
  block_on_stop: true,
};

const ALLOWED_TOOLS =
  "Read,Edit,Write,Glob,Grep,mcp__cliptown__*";

function settingsJson(port: number): object {
  // Each hook script POSTs the matcher payload to the bridge.
  // Claude Code's hook command receives the payload on stdin; we forward it
  // verbatim. `curl --data-binary @-` reads stdin.
  const post = (kind: string) =>
    `curl --silent --max-time 2 --data-binary @- -X POST http://127.0.0.1:${port}/hook/${kind}`;
  return {
    hooks: {
      PreToolUse: [{ matcher: "*", hooks: [{ type: "command", command: post("pre_tool") }] }],
      PostToolUse: [{ matcher: "*", hooks: [{ type: "command", command: post("post_tool") }] }],
      Stop: [{ hooks: [{ type: "command", command: post("session_stop") }] }],
      Notification: [{ hooks: [{ type: "command", command: post("session_error") }] }],
    },
  };
}

function mcpJson(worldUrl: string, token: string): object {
  // M9.10 A1': MCP lives at the world over HTTP, not at a per-worker Unix
  // socket. The CLI POSTs JSON-RPC `tools/call` directly to the world's
  // `/mcp` route with `Authorization: Bearer <agent_id>:<secret>`. See
  // `crates/world/src/mcp_http.rs`.
  return {
    mcpServers: {
      cliptown: {
        type: "http",
        url: `${worldUrl}/mcp`,
        headers: { Authorization: `Bearer ${token}` },
      },
    },
  };
}

async function buildConfig(port: number, worldUrl: string, token: string): Promise<{ cfgDir: string; cleanup: () => Promise<void> }> {
  const cfgDir = await mkdtemp(join(tmpdir(), "ct-cc-"));
  await writeFile(join(cfgDir, "mcp.json"), JSON.stringify(mcpJson(worldUrl, token), null, 2));
  await writeFile(join(cfgDir, "settings.json"), JSON.stringify(settingsJson(port), null, 2));
  return {
    cfgDir,
    async cleanup() {
      await rm(cfgDir, { recursive: true, force: true });
    },
  };
}

export const claudeCodeAdapter: BackendAdapter = {
  id: "claude_code",
  capabilities: CAPS,
  async spawn(opts: SpawnOpts): Promise<SpawnResult> {
    const onHook = opts.onHook ?? (() => { /* noop */ });

    let bridge: HookBridge | null = null;
    let cfg: { cfgDir: string; cleanup: () => Promise<void> } | null = null;

    try {
      bridge = await startHookBridge(onHook);
      cfg = await buildConfig(bridge.port, opts.mcp_world_url, opts.mcp_token);
    } catch (e) {
      if (bridge) await bridge.close();
      if (cfg) await cfg.cleanup();
      throw e;
    }

    const bin = opts.bin ?? process.env.CLIPTOWN_FIXTURE_CLI ?? "claude";
    // Skip `--output-format json` when running against the fixture CLI in
    // contract tests — fixture-cli doesn't speak that flag and emits its own
    // synthetic hook sequence instead. Detected via CLIPTOWN_FIXTURE_CLI or
    // an explicit opts.bin override.
    const useJsonOutput = !opts.bin && !process.env.CLIPTOWN_FIXTURE_CLI;
    const env = {
      ...process.env,
      ...opts.env,
      // Hook scripts/settings live in cfgDir.
      CLAUDE_CODE_SETTINGS: join(cfg.cfgDir, "settings.json"),
    };
    const args = [
      "--print", opts.prompt,
      "--allowedTools", ALLOWED_TOOLS,
      "--mcp-config", join(cfg.cfgDir, "mcp.json"),
      "--strict-mcp-config",
    ];
    if (useJsonOutput) {
      // claude --print --output-format json emits a single JSON object on
      // stdout containing `usage` + `total_cost_usd` + `modelUsage`. We parse
      // it at exit to populate UsageReport, which the worker then forwards
      // to the world as a `report_budget` WS frame (M9.10 budget telemetry).
      args.push("--output-format", "json");
      // claude 2.1.x ignores the CLAUDE_CODE_SETTINGS env var; only the
      // explicit --settings flag wires hooks into the per-spawn settings file.
      // The env var is kept above because the fixture-cli used in contract
      // tests still reads it (its argparse is strict and rejects --settings).
      args.push("--settings", join(cfg.cfgDir, "settings.json"));
    }
    const child = nodeSpawn(
      bin,
      args,
      // stdin=ignore (effectively /dev/null) silences claude's "no stdin data
      // received in 3s" warning that fires when stdin is an open but unwritten
      // pipe. Operator log stays clean; CLI behavior unchanged because we
      // never piped anything in.
      { cwd: opts.cwd, env, stdio: ["ignore", "pipe", "pipe"] },
    );

    // Buffer stdout so we can parse the JSON at exit, while still teeing
    // chunks to onLog for live operator visibility. stderr is tee-only.
    let stdoutBuf = "";
    child.stdout?.on("data", (b: Buffer) => {
      const s = b.toString("utf-8");
      stdoutBuf += s;
      opts.onLog?.("stdout", s);
    });
    if (opts.onLog) {
      child.stderr?.on("data", (b: Buffer) => opts.onLog?.("stderr", b.toString("utf-8")));
    }

    const exit = new Promise<{ exit_code: number; signal?: string }>((resolve) => {
      child.on("exit", (code, signal) => {
        resolve({ exit_code: code ?? -1, signal: signal ?? undefined });
      });
    });

    // Keep refs for cleanup on wait/kill.
    const _bridge = bridge;
    const _cfg = cfg;

    const result: SpawnResult = {
      pid: child.pid ?? -1,
      async wait() {
        const r = await exit;
        await Promise.allSettled([_bridge.close(), _cfg.cleanup()]);
        const usage = useJsonOutput ? parseUsage(stdoutBuf) : undefined;
        return { ...r, usage };
      },
      kill(signal: NodeJS.Signals = "SIGTERM") {
        try { child.kill(signal); } catch { /* noop */ }
      },
    };

    return result;
  },
};

/**
 * Parse the `claude --print --output-format json` result envelope. Returns
 * undefined if the buffer isn't valid JSON or doesn't carry the expected
 * `usage` + `total_cost_usd` shape — defensive because the CLI may evolve
 * the schema or refuse to emit JSON on error paths.
 *
 * `in_tokens` sums input_tokens + cache_creation + cache_read so the world's
 * budget ladder sees all tokens the model billed for, not just net-new
 * prompt bytes. `model_id` comes from `modelUsage`'s sole key (claude-code
 * runs one model per --print invocation).
 */
function parseUsage(buf: string): UsageReport | undefined {
  try {
    const j = JSON.parse(buf) as {
      total_cost_usd?: number;
      usage?: {
        input_tokens?: number;
        output_tokens?: number;
        cache_creation_input_tokens?: number;
        cache_read_input_tokens?: number;
      };
      modelUsage?: Record<string, unknown>;
    };
    if (!j.usage || typeof j.total_cost_usd !== "number") return undefined;
    const u = j.usage;
    const in_tokens =
      (u.input_tokens ?? 0) +
      (u.cache_creation_input_tokens ?? 0) +
      (u.cache_read_input_tokens ?? 0);
    const out_tokens = u.output_tokens ?? 0;
    const model_id = Object.keys(j.modelUsage ?? {})[0] ?? "claude-unknown";
    return { in_tokens, out_tokens, cost_usd: j.total_cost_usd, model_id };
  } catch {
    return undefined;
  }
}

// Re-export for convenience.
export type { HookEvent };
