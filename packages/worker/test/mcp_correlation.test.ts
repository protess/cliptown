import { describe, it, expect } from "vitest";
import {
  callOverWS,
  createMcpProxy,
  MCP_TOOL_NAMES,
} from "../src/mcp.js";
import type { WorkerHandle } from "../src/ws.js";

interface FakeHandle extends WorkerHandle {
  feed(msg: object): void;
  sent: object[];
}

function fakeHandle(): FakeHandle {
  const listeners = new Set<(m: unknown) => void>();
  const sent: object[] = [];
  const handle: FakeHandle = {
    send(msg: object) {
      sent.push(msg);
    },
    onMessage(fn: (m: unknown) => void) {
      listeners.add(fn);
    },
    offMessage(fn: (m: unknown) => void) {
      listeners.delete(fn);
    },
    close() {
      listeners.clear();
    },
    listenerCount() {
      return listeners.size;
    },
    feed(msg: object) {
      // Snapshot to a stable list — a listener that removes itself during
      // dispatch must not perturb the iteration.
      for (const fn of Array.from(listeners)) fn(msg);
    },
    sent,
  };
  return handle;
}

function lastSent(h: FakeHandle): { tool: string; corr_id: string; args: object; type: string } {
  return h.sent[h.sent.length - 1] as {
    tool: string;
    corr_id: string;
    args: object;
    type: string;
  };
}

describe("mcp callOverWS — listener cleanup", () => {
  it("resolves on mcp_reply and removes listener", async () => {
    const h = fakeHandle();
    const p = callOverWS(h, { tool: "speak", args: { kind: "chat", body: "hi" } });
    expect(h.listenerCount()).toBe(1);
    const { corr_id } = lastSent(h);
    h.feed({ type: "mcp_reply", corr_id, result: { ok: true } });
    await expect(p).resolves.toEqual({ ok: true });
    expect(h.listenerCount()).toBe(0);
  });

  it("rejects on mcp_error and removes listener", async () => {
    const h = fakeHandle();
    const p = callOverWS(h, { tool: "speak", args: {} });
    expect(h.listenerCount()).toBe(1);
    const { corr_id } = lastSent(h);
    h.feed({
      type: "mcp_error",
      corr_id,
      code: "permission_denied",
      message: "no",
    });
    await expect(p).rejects.toMatchObject({
      message: "no",
      code: "permission_denied",
    });
    expect(h.listenerCount()).toBe(0);
  });

  it("rejects on timeout and removes listener", async () => {
    const h = fakeHandle();
    const p = callOverWS(h, { tool: "speak", args: {} }, 50);
    expect(h.listenerCount()).toBe(1);
    await expect(p).rejects.toThrow(/mcp_call_timeout/);
    expect(h.listenerCount()).toBe(0);
  });

  it("ignores frames with mismatched corr_id (listener stays)", async () => {
    const h = fakeHandle();
    const p = callOverWS(h, { tool: "speak", args: {} }, 200);
    expect(h.listenerCount()).toBe(1);

    // Stray frame for a different call: must not resolve or remove listener.
    h.feed({ type: "mcp_reply", corr_id: "wrong-id", result: 1 });
    expect(h.listenerCount()).toBe(1);

    const { corr_id } = lastSent(h);
    h.feed({ type: "mcp_reply", corr_id, result: 2 });
    await expect(p).resolves.toBe(2);
    expect(h.listenerCount()).toBe(0);
  });

  it("100 sequential calls do not leak listeners", async () => {
    const h = fakeHandle();
    for (let i = 0; i < 100; i++) {
      const p = callOverWS(h, { tool: "speak", args: { i } });
      const { corr_id } = lastSent(h);
      h.feed({ type: "mcp_reply", corr_id, result: i });
      await expect(p).resolves.toBe(i);
      expect(h.listenerCount()).toBe(0);
    }
    expect(h.sent.length).toBe(100);
  });

  it("100 sequential calls with mixed errors and timeouts do not leak", async () => {
    const h = fakeHandle();
    for (let i = 0; i < 100; i++) {
      const mode = i % 3; // 0 = reply, 1 = error, 2 = timeout
      if (mode === 0) {
        const p = callOverWS(h, { tool: "speak", args: { i } });
        const { corr_id } = lastSent(h);
        h.feed({ type: "mcp_reply", corr_id, result: i });
        await expect(p).resolves.toBe(i);
      } else if (mode === 1) {
        const p = callOverWS(h, { tool: "speak", args: { i } });
        const { corr_id } = lastSent(h);
        h.feed({ type: "mcp_error", corr_id, code: "x", message: "boom" });
        await expect(p).rejects.toThrow(/boom/);
      } else {
        const p = callOverWS(h, { tool: "speak", args: { i } }, 5);
        await expect(p).rejects.toThrow(/mcp_call_timeout/);
      }
      expect(h.listenerCount()).toBe(0);
    }
  });

  it("each call gets a unique corr_id", async () => {
    const h = fakeHandle();
    const ids = new Set<string>();
    for (let i = 0; i < 50; i++) {
      const p = callOverWS(h, { tool: "speak", args: { i } });
      const { corr_id } = lastSent(h);
      ids.add(corr_id);
      h.feed({ type: "mcp_reply", corr_id, result: 0 });
      await p;
    }
    expect(ids.size).toBe(50);
  });
});

describe("mcp createMcpProxy", () => {
  it("exposes all 16 spec §6.2 worker-callable tools", () => {
    const h = fakeHandle();
    const proxy = createMcpProxy(h);
    expect(MCP_TOOL_NAMES.length).toBe(16);
    for (const tool of MCP_TOOL_NAMES) {
      expect(typeof (proxy as unknown as Record<string, unknown>)[tool]).toBe(
        "function",
      );
    }
  });

  it("does NOT expose operator-only tools", () => {
    const proxy = createMcpProxy(fakeHandle());
    expect(
      (proxy as unknown as Record<string, unknown>).operator_force_accept,
    ).toBeUndefined();
    expect(
      (proxy as unknown as Record<string, unknown>).operator_force_fail,
    ).toBeUndefined();
  });

  it("routes proxy method to the right tool name in the mcp_call frame", async () => {
    const h = fakeHandle();
    const proxy = createMcpProxy(h);
    const p = proxy.move_intent({ target_room: "lobby" });
    const sent = lastSent(h);
    expect(sent.type).toBe("mcp_call");
    expect(sent.tool).toBe("move_intent");
    expect(sent.args).toEqual({ target_room: "lobby" });
    h.feed({ type: "mcp_reply", corr_id: sent.corr_id, result: "ok" });
    await expect(p).resolves.toBe("ok");
    expect(h.listenerCount()).toBe(0);
  });

  it("each proxy method emits a fresh corr_id and cleans up", async () => {
    const h = fakeHandle();
    const proxy = createMcpProxy(h);
    const p1 = proxy.speak({ kind: "chat", body: "a" });
    const p2 = proxy.speak({ kind: "chat", body: "b" });
    expect(h.listenerCount()).toBe(2);
    const sent1 = h.sent[0] as { corr_id: string };
    const sent2 = h.sent[1] as { corr_id: string };
    expect(sent1.corr_id).not.toBe(sent2.corr_id);
    h.feed({ type: "mcp_reply", corr_id: sent2.corr_id, result: 2 });
    h.feed({ type: "mcp_reply", corr_id: sent1.corr_id, result: 1 });
    await expect(p1).resolves.toBe(1);
    await expect(p2).resolves.toBe(2);
    expect(h.listenerCount()).toBe(0);
  });
});
