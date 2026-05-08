import { describe, it, expect } from "vitest";
import type { BackendAdapter, SpawnOpts, SpawnResult, HookEvent } from "../src/index.js";

describe("BackendAdapter contract", () => {
  it("a minimal stub conforms to the interface", async () => {
    const stub: BackendAdapter = {
      id: "claude_code",
      capabilities: {
        hooks: ["pre_tool", "post_tool", "session_stop", "session_error"],
        inject_context: true,
        block_on_stop: true,
      },
      async spawn(opts: SpawnOpts): Promise<SpawnResult> {
        // SpawnOpts.mcp_socket_path is mandatory — TS would reject if missing.
        const _path: string = opts.mcp_socket_path;
        return {
          pid: 1234,
          wait: async () => ({ exit_code: 0 }),
          kill: () => { /* noop */ },
        };
      },
    };
    expect(stub.id).toBe("claude_code");
    expect(stub.capabilities.hooks).toContain("pre_tool");
    expect(stub.capabilities.inject_context).toBe(true);
    const r = await stub.spawn({
      prompt: "hi",
      cwd: "/tmp",
      mcp_socket_path: "/tmp/sock",
    });
    expect(r.pid).toBe(1234);
    const exit = await r.wait();
    expect(exit.exit_code).toBe(0);
  });

  it("HookEvent shape covers the four kinds", () => {
    const e: HookEvent = {
      kind: "pre_tool",
      tool: "bash",
      payload: { cmd: "ls" },
      seq: 1,
      ts_ms: Date.now(),
    };
    expect(e.kind).toBe("pre_tool");
  });

  it("SpawnOpts.mcp_socket_path is required (compile-time check via assignment)", () => {
    // If this line compiles, the field is required (TS rejects missing required fields).
    const opts: SpawnOpts = {
      prompt: "x",
      cwd: "/tmp",
      mcp_socket_path: "/tmp/sock",
    };
    expect(opts.mcp_socket_path).toBe("/tmp/sock");
  });
});
