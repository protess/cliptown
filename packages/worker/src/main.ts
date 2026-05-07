import { parseArgs } from "node:util";
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve as pathResolve } from "node:path";
import { connect, type WorkerHandle } from "./ws.js";
import { createMcpProxy, type McpProxy, MCP_TOOL_NAMES } from "./mcp.js";
import { LLMMock, type ToolUse } from "./llm_mock.js";
import { resolveSandbox } from "./sandbox.js";

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
  };
}

/** Run a single tool_use against the MCP proxy or local fs (writeFile). */
async function runToolUse(
  tu: ToolUse,
  proxy: McpProxy,
  workspaceRoot: string,
): Promise<void> {
  switch (tu.kind) {
    case "mcp": {
      if (!(MCP_TOOL_NAMES as readonly string[]).includes(tu.tool)) {
        throw new Error(`unknown MCP tool: ${tu.tool}`);
      }
      const fn = (proxy as unknown as Record<string, (a: Record<string, unknown>) => Promise<unknown>>)[tu.tool];
      await fn(tu.args);
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

  const handle: WorkerHandle = await connect({
    url:       args.worldUrl,
    agentId:   args.agentId,
    startupId: args.startupId,
    secret:    args.secret,
  });
  console.log(`[worker] connected; waiting for task_assigned`);

  const proxy = createMcpProxy(handle);
  const workspaceRoot = pathResolve(args.workspace);

  // Inbound frame logger (Phase 0): just print non-MCP frames so we see
  // task_assigned, directive, move_complete, etc. The MCP correlation layer
  // already filters mcp_reply/mcp_error by corr_id.
  handle.onMessage((m) => {
    const o = m as { type?: string; corr_id?: string };
    if (typeof o?.type === "string" && o.type !== "mcp_reply" && o.type !== "mcp_error") {
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
        await runToolUse(tu, proxy, workspaceRoot);
      } catch (e) {
        console.error(`[worker] tool_use failed:`, e);
        break;
      }
    }
    console.log(`[worker] mock sequence complete; idling for inbound frames`);
  }

  // Stay alive for inbound frames until SIGINT or WS close.
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
  });
}

// Only run main when invoked directly (not when imported by tests).
if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((e) => {
    console.error(`[worker] fatal:`, e);
    process.exit(1);
  });
}
