import { describe, it, expect } from "vitest";
import { startHookBridge } from "@cliptown/adapter-core";
import type { HookEvent } from "@cliptown/adapter-core";

async function postJson(port: number, path: string, body: unknown): Promise<{ ok: boolean }> {
  const res = await fetch(`http://127.0.0.1:${port}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });
  return res.json() as Promise<{ ok: boolean }>;
}

describe("hook bridge", () => {
  it("normalizes pre_tool POST into HookEvent", async () => {
    const events: HookEvent[] = [];
    const b = await startHookBridge((e) => events.push(e));
    const r = await postJson(b.port, "/hook/pre_tool", { tool: "bash", args: { cmd: "ls" } });
    expect(r.ok).toBe(true);
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({ kind: "pre_tool", tool: "bash", payload: { tool: "bash", args: { cmd: "ls" } } });
    expect(events[0].seq).toBe(1);
    expect(typeof events[0].ts_ms).toBe("number");
    await b.close();
  });

  it("supports the four hook kinds and increments seq monotonically", async () => {
    const events: HookEvent[] = [];
    const b = await startHookBridge((e) => events.push(e));
    await postJson(b.port, "/hook/pre_tool", { tool: "x" });
    await postJson(b.port, "/hook/post_tool", { tool: "x", result: "ok" });
    await postJson(b.port, "/hook/session_stop", { reason: "natural" });
    await postJson(b.port, "/hook/session_error", { reason: "timeout" });
    expect(events.map(e => e.kind)).toEqual(["pre_tool", "post_tool", "session_stop", "session_error"]);
    expect(events.map(e => e.seq)).toEqual([1, 2, 3, 4]);
    await b.close();
  });

  it("returns 404 for unknown hook kind", async () => {
    const b = await startHookBridge(() => { /* noop */ });
    const res = await fetch(`http://127.0.0.1:${b.port}/hook/garbage`, { method: "POST", body: "{}" });
    expect(res.status).toBe(404);
    await b.close();
  });

  it("returns 405 for non-POST", async () => {
    const b = await startHookBridge(() => { /* noop */ });
    const res = await fetch(`http://127.0.0.1:${b.port}/hook/pre_tool`);
    expect(res.status).toBe(405);
    await b.close();
  });

  it("survives consumer callback throwing", async () => {
    const b = await startHookBridge(() => { throw new Error("boom"); });
    const r = await postJson(b.port, "/hook/pre_tool", { tool: "x" });
    expect(r.ok).toBe(true);
    await b.close();
  });

  it("close() releases the port", async () => {
    const b = await startHookBridge(() => { /* noop */ });
    await b.close();
    await expect(fetch(`http://127.0.0.1:${b.port}/hook/pre_tool`, { method: "POST", body: "{}" }))
      .rejects.toThrow();
  });

  it("prefers tool_name over tool field (claude CLI payload shape)", async () => {
    const events: HookEvent[] = [];
    const b = await startHookBridge((e) => events.push(e));
    const r = await postJson(b.port, "/hook/pre_tool", {
      hook_event_name: "PreToolUse",
      tool_name: "Write",
      tool_input: { file_path: "/tmp/x.txt", content: "hi" },
    });
    expect(r.ok).toBe(true);
    expect(events).toHaveLength(1);
    expect(events[0].tool).toBe("Write");
    await b.close();
  });
});

describe("claudeCodeAdapter", () => {
  it("exports BackendAdapter with id claude_code and full hooks list", async () => {
    const { claudeCodeAdapter } = await import("../src/index.js");
    expect(claudeCodeAdapter.id).toBe("claude_code");
    expect(claudeCodeAdapter.capabilities.hooks).toEqual([
      "pre_tool", "post_tool", "session_stop", "session_error",
    ]);
    expect(claudeCodeAdapter.capabilities.inject_context).toBe(true);
    expect(claudeCodeAdapter.capabilities.block_on_stop).toBe(true);
  });
});
