import { spawn as nodeSpawn } from "node:child_process";
import { writeFile, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  startHookBridge,
  type AdapterCapabilities, type BackendAdapter, type HookBridge,
  type HookEvent, type SpawnOpts, type SpawnResult,
} from "@cliptown/adapter-core";

/**
 * opencode CLI adapter. Phase 0 simplifications:
 *   - block_on_stop: false (no stop-blocking mechanism comparable to Claude Code).
 *   - inject_context: true — opencode supports session resume, so the worker can
 *     prepend additional context across turns by re-spawning with a resume id.
 *     Phase 0 only advertises the capability; the resume wiring lives in M9+.
 *   - MCP config emitted as `mcp_servers` JSON pointing at the worker socket.
 *   - Provider routing: opencode supports multiple LLM providers
 *     (anthropic / openai / etc.). Phase 0 reads OPENCODE_PROVIDER from
 *     SpawnOpts.env or process.env (default "anthropic") and forwards it to
 *     the CLI via `--provider <id>` plus the same env var so providers that
 *     pull config from env still see it.
 *   - Hooks bridge: shared with core; opencode's hook protocol differs (TODO M9+).
 *     Phase 0 advertises only session_stop / session_error.
 *
 * Override the binary via SpawnOpts.bin or CLIPTOWN_FIXTURE_CLI for tests.
 */

const CAPS: AdapterCapabilities = {
  hooks: ["session_stop", "session_error"],
  inject_context: true,
  block_on_stop: false,
};

const PROVIDER_ENV = "OPENCODE_PROVIDER";
const DEFAULT_PROVIDER = "anthropic";

function mcpJson(worldUrl: string, token: string): object {
  // M9.10 A1' — MCP-at-the-world. Opencode's actual MCP config key may differ;
  // the shape below is contract-compatible and matches the core `SpawnOpts`
  // exposure. Real opencode MCP wiring lands when this adapter is exercised
  // end-to-end.
  return {
    mcp_servers: {
      cliptown: {
        type: "http",
        url: `${worldUrl}/mcp`,
        headers: { Authorization: `Bearer ${token}` },
      },
    },
  };
}

async function buildConfig(worldUrl: string, token: string): Promise<{ cfgDir: string; cleanup: () => Promise<void> }> {
  const cfgDir = await mkdtemp(join(tmpdir(), "ct-opencode-"));
  await writeFile(join(cfgDir, "config.json"), JSON.stringify(mcpJson(worldUrl, token), null, 2));
  return {
    cfgDir,
    async cleanup() {
      await rm(cfgDir, { recursive: true, force: true });
    },
  };
}

export const opencodeAdapter: BackendAdapter = {
  id: "opencode",
  capabilities: CAPS,
  async spawn(opts: SpawnOpts): Promise<SpawnResult> {
    const onHook = opts.onHook ?? (() => { /* noop */ });

    let bridge: HookBridge | null = null;
    let cfg: { cfgDir: string; cleanup: () => Promise<void> } | null = null;

    try {
      bridge = await startHookBridge(onHook);
      cfg = await buildConfig(opts.mcp_world_url, opts.mcp_token);
    } catch (e) {
      if (bridge) await bridge.close();
      if (cfg) await cfg.cleanup();
      throw e;
    }

    const bin = opts.bin ?? process.env.CLIPTOWN_FIXTURE_CLI ?? "opencode";
    const provider = opts.env?.[PROVIDER_ENV] ?? process.env[PROVIDER_ENV] ?? DEFAULT_PROVIDER;
    const cfgPath = join(cfg.cfgDir, "config.json");
    const env = {
      ...process.env,
      ...opts.env,
      OPENCODE_HOOK_PORT: String(bridge.port),
      OPENCODE_CONFIG: cfgPath,
      [PROVIDER_ENV]: provider,
    };
    const child = nodeSpawn(
      bin,
      ["--prompt", opts.prompt, "--config", cfgPath, "--provider", provider],
      { cwd: opts.cwd, env, stdio: ["pipe", "pipe", "pipe"] },
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

    const _bridge = bridge;
    const _cfg = cfg;

    const result: SpawnResult = {
      pid: child.pid ?? -1,
      async wait() {
        const r = await exit;
        await Promise.allSettled([_bridge.close(), _cfg.cleanup()]);
        return r;
      },
      kill(signal: NodeJS.Signals = "SIGTERM") {
        try { child.kill(signal); } catch { /* noop */ }
      },
    };

    return result;
  },
};

export type { HookEvent };
