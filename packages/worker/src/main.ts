import { parseArgs } from "node:util";
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve as pathResolve } from "node:path";
import { connect, type WorkerHandle } from "./ws.js";
import { createMcpProxy, type McpProxy, MCP_TOOL_NAMES } from "./mcp.js";
import { LLMMock, type ToolUse } from "./llm_mock.js";
import { resolveSandbox } from "./sandbox.js";
import type { BackendAdapter } from "@cliptown/adapter-core";
import { claudeCodeAdapter } from "@cliptown/adapter-claude-code";

export interface ParsedArgs {
  worldUrl: string;
  agentId: string;
  startupId: string;
  secret: string;
  backend: string;
  workspace: string;
  mock: boolean;
  fixture: string | undefined;
  prompt: string;
  /**
   * M9.10 A2: one-shot real-LLM mode. When set, the worker connects to WS,
   * spawns the picked backend adapter (which talks to the world's `/mcp`
   * HTTP endpoint directly per A1'), waits for the CLI to exit, then closes
   * the WS and exits. Mutually exclusive with `--mock`.
   */
  real: boolean;
}

export function parseWorkerArgs(argv: string[]): ParsedArgs {
  const { values } = parseArgs({
    args: argv,
    options: {
      "world-url":  { type: "string" },
      "agent-id":   { type: "string" },
      "startup-id": { type: "string" },
      "secret":     { type: "string" },
      "backend":    { type: "string", default: "claude_code" },
      "workspace":  { type: "string" },
      "mock":       { type: "boolean", default: false },
      "fixture":    { type: "string" },
      "prompt":     { type: "string", default: "" },
      "real":       { type: "boolean", default: false },
    },
    strict: true,
    allowPositionals: false,
  });

  const required = (k: string, v: unknown): string => {
    if (typeof v !== "string" || v.length === 0) {
      throw new Error(`missing required arg --${k}`);
    }
    return v;
  };

  return {
    worldUrl:  required("world-url",  values["world-url"]),
    agentId:   required("agent-id",   values["agent-id"]),
    startupId: required("startup-id", values["startup-id"]),
    secret:    required("secret",     values["secret"]),
    backend:   String(values["backend"]),
    workspace: required("workspace",  values["workspace"]),
    mock:      Boolean(values["mock"]),
    fixture:   typeof values["fixture"] === "string" ? values["fixture"] : undefined,
    prompt:    String(values["prompt"]),
    real:      Boolean(values["real"]),
  };
}

/**
 * Recursively replace `__STARTUP_ID__` in any string field of an mcp args
 * object with the worker's actual startup id. The fixture format is
 * generic (it doesn't know which startup it'll run under at emit-time), so
 * paths like `workspaces/__STARTUP_ID__/artifacts/T1.md` are rewritten here
 * before they reach `mcp_dispatch::handle_task_done`'s canonical-path check.
 *
 * Walks nested objects but treats arrays as opaque (no current fixture uses
 * arrays of strings as args). Non-string scalars pass through unchanged.
 */
export function substitutePlaceholders(
  args: Record<string, unknown>,
  startupId: string,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(args)) {
    if (typeof v === "string") {
      out[k] = v.replace(/__STARTUP_ID__/g, startupId);
    } else if (v && typeof v === "object" && !Array.isArray(v)) {
      out[k] = substitutePlaceholders(v as Record<string, unknown>, startupId);
    } else {
      out[k] = v;
    }
  }
  return out;
}

/**
 * Pick the BackendAdapter for the requested CLI. M9.10 A2 lands `claude_code`
 * first; codex/opencode are wired through A1' (their `mcp.json` shape already
 * matches) but driving them end-to-end isn't in scope for this milestone, so
 * we throw a discoverable error rather than silently using a half-tested path.
 */
export function pickAdapter(backend: string): BackendAdapter {
  switch (backend) {
    case "claude_code":
      return claudeCodeAdapter;
    case "codex":
    case "opencode":
      throw new Error(
        `backend "${backend}" not_yet_supported_in_real_mode — M9.10 A2 lands claude_code first`,
      );
    default:
      throw new Error(`unknown backend: ${backend}`);
  }
}

/** Run a single tool_use against the MCP proxy or local fs (writeFile). */
export async function runToolUse(
  tu: ToolUse,
  proxy: McpProxy,
  workspaceRoot: string,
  startupId: string,
): Promise<void> {
  switch (tu.kind) {
    case "mcp": {
      if (!(MCP_TOOL_NAMES as readonly string[]).includes(tu.tool)) {
        throw new Error(`unknown MCP tool: ${tu.tool}`);
      }
      const fn = (proxy as unknown as Record<string, (a: Record<string, unknown>) => Promise<unknown>>)[tu.tool];
      await fn(substitutePlaceholders(tu.args, startupId));
      return;
    }
    case "writeFile": {
      const abs = resolveSandbox(workspaceRoot, tu.path);
      await mkdir(dirname(abs), { recursive: true });
      await writeFile(abs, tu.content, "utf-8");
      return;
    }
    case "done":
      return;
  }
}

async function main(): Promise<void> {
  const args = parseWorkerArgs(process.argv.slice(2));
  console.log(`[worker] connecting to ${args.worldUrl} as ${args.agentId} (startup=${args.startupId})`);

  // Resolve once when the WS closes (e.g. world disconnected). Without this
  // the worker would keep running orphaned and the supervisor would never
  // observe an exit code.
  let resolveOnClose: (() => void) | null = null;
  const closedPromise = new Promise<void>((r) => {
    resolveOnClose = r;
  });

  const handle: WorkerHandle = await connect({
    url:       args.worldUrl,
    agentId:   args.agentId,
    startupId: args.startupId,
    secret:    args.secret,
    onClose: () => {
      console.log(`[worker] WS closed by world; exiting`);
      resolveOnClose?.();
    },
  });
  console.log(`[worker] connected; waiting for task_assigned`);

  const proxy = createMcpProxy(handle);
  const workspaceRoot = pathResolve(args.workspace);

  // Inbound frame logger (Phase 0): print non-MCP, non-heartbeat frames so we
  // see task_assigned, directive, move_complete, etc. The MCP correlation layer
  // already filters mcp_reply/mcp_error by corr_id. Heartbeat-style frames
  // (proximity_tick at 1Hz) would drown the actual control events.
  const HEARTBEAT_TYPES = new Set(["proximity_tick", "mcp_reply", "mcp_error"]);
  handle.onMessage((m) => {
    const o = m as { type?: string };
    if (typeof o?.type === "string" && !HEARTBEAT_TYPES.has(o.type)) {
      console.log(`[worker] inbound: ${JSON.stringify(o)}`);
    }
  });

  // Mock mode: drive the canned tool_use sequence on connect.
  if (args.mock) {
    const mock = new LLMMock({ defaultFixture: args.fixture ?? "engineer_writes_spec" });
    mock.setPrompt(args.prompt);
    while (true) {
      const tu = mock.next();
      if (tu === null) break;
      try {
        await runToolUse(tu, proxy, workspaceRoot, args.startupId);
      } catch (e) {
        console.error(`[worker] tool_use failed:`, e);
        break;
      }
    }
    console.log(`[worker] mock sequence complete; idling for inbound frames`);
  } else if (args.real) {
    // M9.10 A2 — one-shot real-LLM mode. Worker becomes a process supervisor:
    // spawn the adapter, log hooks + stdio, wait for CLI exit, close WS, done.
    // MCP traffic flows CLI → world `/mcp` (HTTP) directly per A1' — the
    // worker's `McpProxy` is unused in this path.
    const adapter = pickAdapter(args.backend);
    // `args.worldUrl` is the WS endpoint (ws://host:port/ws/worker). The HTTP
    // base for `/mcp` strips the path and switches scheme. wss:// → https://
    // works too because we replace the literal `ws` prefix.
    const httpBase = new URL(args.worldUrl);
    httpBase.protocol = httpBase.protocol === "wss:" ? "https:" : "http:";
    httpBase.pathname = "";
    httpBase.search = "";
    httpBase.hash = "";
    const mcpWorldUrl = httpBase.toString().replace(/\/$/, "");
    // Per `crates/world/src/mcp_http.rs::authenticate`, the bearer token is
    // `<agent_id>:<secret>` so the world can resolve which agent is calling
    // without a separate header.
    const mcpToken = `${args.agentId}:${args.secret}`;
    console.log(
      `[worker] real mode: spawning ${args.backend} → MCP @ ${mcpWorldUrl}/mcp`,
    );
    const spawned = await adapter.spawn({
      prompt: args.prompt,
      cwd: workspaceRoot,
      mcp_world_url: mcpWorldUrl,
      mcp_token: mcpToken,
      onHook: (e) => console.log(`[worker] hook: ${e.kind} tool=${e.tool}`),
      onLog: (stream, line) => {
        const out = stream === "stderr" ? process.stderr : process.stdout;
        out.write(`[${args.backend}] ${line}`);
      },
    });
    const exit = await spawned.wait();
    console.log(
      `[worker] adapter exited code=${exit.exit_code} signal=${exit.signal ?? "none"}`,
    );
    handle.close();
    return;
  }

  // Stay alive until SIGINT, SIGTERM, OR the world closes the WS.
  await new Promise<void>((resolve) => {
    process.once("SIGINT", () => {
      console.log(`[worker] SIGINT — closing`);
      handle.close();
      resolve();
    });
    process.once("SIGTERM", () => {
      handle.close();
      resolve();
    });
    closedPromise.then(() => resolve());
  });
}

// Only run main when invoked directly (not when imported by tests).
if (import.meta.url === `file://${process.argv[1]}`) {
  main()
    .then(() => {
      // Ensure node exits cleanly so the supervisor sees the exit code
      // instead of the process lingering on stray timers/listeners.
      process.exit(0);
    })
    .catch((e) => {
      console.error(`[worker] fatal:`, e);
      process.exit(1);
    });
}
