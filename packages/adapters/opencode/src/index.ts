import { spawn as nodeSpawn } from "node:child_process";
import { writeFile, rm } from "node:fs/promises";
import { join } from "node:path";
import {
  type AdapterCapabilities, type BackendAdapter,
  type HookEvent, type SpawnOpts, type SpawnResult, type UsageReport,
} from "@cliptown/adapter-core";
import { startServe } from "./serve_lifecycle.js";
import { subscribeSse } from "./sse_client.js";
import { emptyMapState, mapEvent, toUsageReport, type MapState } from "./event_mapper.js";
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
            seq: ++mapState.seq,
            ts_ms: Date.now(),
          });
        }
      } finally {
        // Always resolve idle so wait() never deadlocks. Whether SSE ended
        // via session.idle, an abort during kill(), an unexpected error, or
        // a server crash, the wait()-blocks-on-idle promise must complete.
        // resolveIdle is idempotent (Promise.resolve always is) so calling
        // it multiple times across paths is safe.
        resolveIdle();
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
        const usage: UsageReport | undefined = toUsageReport(mapState, modelSpec);
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
