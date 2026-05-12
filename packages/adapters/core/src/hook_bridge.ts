import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import type { HookEvent, HookKind } from "./index.js";

/**
 * Adapter-agnostic HTTP listener that LLM CLI hook scripts POST into.
 * Each spawn() call gets its own bridge bound to a random port; the spawning
 * adapter wires the CLI's hook config to POST to that port.
 *
 * Endpoint: POST /hook/:kind  (kind ∈ {pre_tool, post_tool, session_stop, session_error})
 * Body: JSON object — bridge normalizes to HookEvent and forwards to onHook.
 *
 * Lifted from packages/adapters/claude-code/src/hook_bridge.ts in M8.1 so
 * codex + opencode adapters can share the same normalization path.
 */

export interface HookBridge {
  port: number;
  /** Stop the listener and release the port. Idempotent. */
  close(): Promise<void>;
}

const HOOK_KINDS: ReadonlyArray<HookKind> = [
  "pre_tool",
  "post_tool",
  "session_stop",
  "session_error",
];

export async function startHookBridge(
  onHook: (e: HookEvent) => void,
): Promise<HookBridge> {
  let seq = 0;
  const server: Server = createServer((req, res) => {
    if (req.method !== "POST" || !req.url) {
      res.writeHead(405); res.end(); return;
    }
    const m = /^\/hook\/(pre_tool|post_tool|session_stop|session_error)$/.exec(req.url);
    if (!m) {
      res.writeHead(404); res.end(); return;
    }
    const kind = m[1] as HookKind;
    let body = "";
    req.on("data", (chunk: Buffer) => { body += chunk.toString("utf-8"); });
    req.on("end", () => {
      let payload: unknown = null;
      try { payload = body ? JSON.parse(body) : null; } catch { /* fall through with raw */ }
      seq += 1;
      const tool = extractToolName(payload);
      const event: HookEvent = {
        kind,
        tool,
        payload,
        seq,
        ts_ms: Date.now(),
      };
      try { onHook(event); } catch { /* don't crash bridge on consumer error */ }
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ ok: true, seq }));
    });
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve()));
  const addr = server.address() as AddressInfo;

  return {
    port: addr.port,
    async close() {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

function isObj(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function extractToolName(payload: unknown): string {
  if (!isObj(payload)) return "";
  if (typeof payload.tool_name === "string") return payload.tool_name;
  if (typeof payload.tool === "string") return payload.tool;
  return "";
}

export const _internal = { HOOK_KINDS };
