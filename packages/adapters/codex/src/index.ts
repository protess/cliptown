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
 * Codex CLI adapter. Phase 0 simplifications:
 *   - block_on_stop: false (codex has no stop-blocking mechanism comparable to
 *     Claude Code's Stop hook; the worker treats stop as best-effort).
 *   - inject_context: false (no resume-style prompt injection for Phase 0).
 *   - MCP config points the CLI at the world's `/mcp` HTTP endpoint
 *     (M9.10 A1' — MCP-at-the-world). Codex's actual MCP config key may
 *     differ; the shape below is contract-compatible and matches what the
 *     core `SpawnOpts` exposes. Real Codex MCP wiring lands when this
 *     adapter is exercised end-to-end.
 *   - Hooks bridge: codex's hook protocol differs from Claude Code's (TODO M9+);
 *     Phase 0 reuses the shared HTTP bridge and exposes the bridge port via
 *     `CODEX_HOOK_PORT` so the CLI (or a wrapper) can POST normalized events.
 *     Per-tool pre/post hooks are out of scope for Phase 0; the capability list
 *     advertises only `session_stop` / `session_error`.
 *
 * Override the binary via SpawnOpts.bin or CLIPTOWN_FIXTURE_CLI for tests.
 */

const CAPS: AdapterCapabilities = {
  hooks: ["session_stop", "session_error"],
  inject_context: false,
  block_on_stop: false,
};

function mcpJson(worldUrl: string, token: string): object {
  return {
    mcp: {
      cliptown: {
        type: "http",
        url: `${worldUrl}/mcp`,
        headers: { Authorization: `Bearer ${token}` },
      },
    },
  };
}

async function buildConfig(worldUrl: string, token: string): Promise<{ cfgDir: string; cleanup: () => Promise<void> }> {
  const cfgDir = await mkdtemp(join(tmpdir(), "ct-codex-"));
  await writeFile(join(cfgDir, "config.json"), JSON.stringify(mcpJson(worldUrl, token), null, 2));
  return {
    cfgDir,
    async cleanup() {
      await rm(cfgDir, { recursive: true, force: true });
    },
  };
}

export const codexAdapter: BackendAdapter = {
  id: "codex",
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

    const bin = opts.bin ?? process.env.CLIPTOWN_FIXTURE_CLI ?? "codex";
    const cfgPath = join(cfg.cfgDir, "config.json");
    const env = {
      ...process.env,
      ...opts.env,
      CODEX_HOOK_PORT: String(bridge.port),
      CODEX_CONFIG: cfgPath,
    };
    const child = nodeSpawn(
      bin,
      ["--prompt", opts.prompt, "--config", cfgPath],
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
