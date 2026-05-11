import { spawn as nodeSpawn } from "node:child_process";
import { writeFile, rm } from "node:fs/promises";
import { join } from "node:path";
import {
  startHookBridge,
  type AdapterCapabilities, type BackendAdapter, type HookBridge,
  type HookEvent, type SpawnOpts, type SpawnResult, type UsageReport,
} from "@cliptown/adapter-core";

/**
 * opencode CLI adapter (opencode ≥ 1.4). Drives `opencode run` in JSON
 * streaming mode so the worker can run an agent end-to-end via MCP HTTP
 * to the world.
 *
 * Phase 1 (M9.10 follow-up) integration choices:
 *   - MCP is configured via a per-spawn `opencode.json` written into
 *     `opts.cwd` (the workspace the agent operates in). opencode reads
 *     this file from `--dir`, which we also point at `opts.cwd` so the
 *     agent's filesystem context and our MCP config live together. The
 *     bearer token sits inside the JSON's `headers` map because opencode
 *     has no env-var indirection for remote-MCP headers. The file is
 *     removed on exit; if anything else writes to `opencode.json` in
 *     `opts.cwd` we'd clobber it, but that's the workspace's choice.
 *   - `--pure` skips external plugins; safe for the smoke since MCP servers
 *     are configured directly in the JSON, not via plugins.
 *   - `--format json` makes stdout a stream of `step_start`, `text`,
 *     `tool`, `step_finish` events. The adapter scrapes the LAST
 *     `step_finish` event's `tokens` + `cost` for `UsageReport`.
 *   - `--dangerously-skip-permissions` auto-approves tool calls; the
 *     external sandbox (SMOKE_DIR tmpdir + per-spawn bearer token + world's
 *     `mcp_http::authenticate` gate) is the real enforcement boundary.
 *   - `--model <provider/model>`: opencode requires an explicit model
 *     when invoked non-interactively. Default is `openai/gpt-5.4-mini`
 *     (cheap + fast). Override with `OPENCODE_MODEL` in `opts.env`.
 *   - Hook bridge: opencode does not implement Claude Code's per-tool hook
 *     webhook contract, so this adapter advertises only
 *     `session_stop` / `session_error`. The bridge port is started anyway
 *     and exposed via `OPENCODE_HOOK_PORT` so future wiring has a target.
 *
 * Override the binary via `SpawnOpts.bin` or `CLIPTOWN_FIXTURE_CLI` for
 * contract tests against the fixture-cli shim.
 */

const CAPS: AdapterCapabilities = {
  hooks: ["session_stop", "session_error"],
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

export const opencodeAdapter: BackendAdapter = {
  id: "opencode",
  capabilities: CAPS,
  async spawn(opts: SpawnOpts): Promise<SpawnResult> {
    const onHook = opts.onHook ?? (() => { /* noop */ });
    const model = opts.env?.[MODEL_ENV] ?? process.env[MODEL_ENV] ?? DEFAULT_MODEL;

    let bridge: HookBridge | null = null;
    let cfg: { cleanup: () => Promise<void> } | null = null;

    try {
      bridge = await startHookBridge(onHook);
      cfg = await writeOpencodeConfig(opts.cwd, opts.mcp_world_url, opts.mcp_token, model);
    } catch (e) {
      if (bridge) await bridge.close();
      if (cfg) await cfg.cleanup();
      throw e;
    }

    const bin = opts.bin ?? process.env.CLIPTOWN_FIXTURE_CLI ?? "opencode";
    const isFixture = !!(opts.bin || process.env.CLIPTOWN_FIXTURE_CLI);
    const env = {
      ...process.env,
      ...opts.env,
      OPENCODE_HOOK_PORT: String(bridge.port),
      [MODEL_ENV]: model,
    };

    let args: string[];
    if (isFixture) {
      args = ["--prompt", opts.prompt];
    } else {
      args = [
        "run",
        "--pure",
        "--format", "json",
        "--dir", opts.cwd,
        "--dangerously-skip-permissions",
        opts.prompt,
      ];
    }

    const child = nodeSpawn(
      bin,
      args,
      { cwd: opts.cwd, env, stdio: ["ignore", "pipe", "pipe"] },
    );

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

    const _bridge = bridge;
    const _cfg = cfg;

    const result: SpawnResult = {
      pid: child.pid ?? -1,
      async wait() {
        const r = await exit;
        await Promise.allSettled([_bridge.close(), _cfg.cleanup()]);
        const usage = isFixture ? undefined : parseUsage(stdoutBuf, model);
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
 * Parse opencode's JSON event stream for the final `step_finish` event's
 * `tokens` + `cost`. opencode emits one `step_finish` per turn; if the
 * agent does multiple turns we use the last one (cumulative tokens are
 * NOT reported, so we sum across step_finish events to be safe). `cost`
 * is opencode-reported USD and gets passed through as `cost_usd` so the
 * world's budget ladder uses it directly.
 */
function parseUsage(buf: string, model: string): UsageReport | undefined {
  let in_tokens = 0;
  let out_tokens = 0;
  let cost_usd = 0;
  let saw = false;
  for (const line of buf.split("\n")) {
    if (!line.trim()) continue;
    let evt: {
      type?: string;
      part?: {
        tokens?: {
          input?: number;
          output?: number;
          reasoning?: number;
          cache?: { write?: number; read?: number };
        };
        cost?: number;
      };
    };
    try {
      evt = JSON.parse(line);
    } catch {
      continue;
    }
    if (evt.type === "step_finish" && evt.part?.tokens) {
      const t = evt.part.tokens;
      in_tokens +=
        (t.input ?? 0) +
        (t.cache?.write ?? 0) +
        (t.cache?.read ?? 0);
      out_tokens += (t.output ?? 0) + (t.reasoning ?? 0);
      cost_usd += evt.part.cost ?? 0;
      saw = true;
    }
  }
  if (!saw) return undefined;
  return { in_tokens, out_tokens, cost_usd, model_id: model };
}

export type { HookEvent };
