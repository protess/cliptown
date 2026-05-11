import { describe, it, expect } from "vitest";
import { emptyMapState, mapEvent, type MapState } from "../src/event_mapper.js";

function partTool(callID: string, tool: string, status: string, input: unknown = null, output: unknown = null) {
  return {
    type: "message.part.updated" as const,
    properties: {
      part: {
        type: "tool",
        tool,
        callID,
        state: {
          status,
          input,
          output,
          time: { start: 1, end: 2 },
        },
      },
    },
  };
}

describe("opencode event_mapper", () => {
  it("emits pre_tool on first running and post_tool on first completed", () => {
    const state = emptyMapState();
    const events = [
      partTool("c1", "bash", "pending"),
      partTool("c1", "bash", "running", { command: "echo hi" }),
      partTool("c1", "bash", "running", { command: "echo hi" }), // dedup
      partTool("c1", "bash", "running", { command: "echo hi" }),
      partTool("c1", "bash", "completed", { command: "echo hi" }, "hi\n"),
    ];
    const hooks = events.flatMap((e) => mapEvent(e, state).hooks);
    expect(hooks.map((h) => [h.kind, h.tool])).toEqual([
      ["pre_tool", "bash"],
      ["post_tool", "bash"],
    ]);
    expect(hooks[0].payload).toMatchObject({ command: "echo hi" });
    expect(hooks[1].payload).toMatchObject({ output: "hi\n" });
    expect(hooks[0].seq).toBe(1);
    expect(hooks[1].seq).toBe(2);
  });

  it("treats failed as terminal and emits post_tool exactly once", () => {
    const state = emptyMapState();
    const events = [
      partTool("c1", "read", "running"),
      partTool("c1", "read", "failed", null, { error: "ENOENT" }),
      partTool("c1", "read", "failed", null, { error: "ENOENT" }), // dedup
    ];
    const hooks = events.flatMap((e) => mapEvent(e, state).hooks);
    expect(hooks.map((h) => h.kind)).toEqual(["pre_tool", "post_tool"]);
  });

  it("interleaves two concurrent tool calls cleanly", () => {
    const state = emptyMapState();
    const events = [
      partTool("c1", "bash", "running"),
      partTool("c2", "read", "running"),
      partTool("c1", "bash", "completed"),
      partTool("c2", "read", "completed"),
    ];
    const hooks = events.flatMap((e) => mapEvent(e, state).hooks);
    expect(hooks.map((h) => [h.kind, h.tool])).toEqual([
      ["pre_tool", "bash"],
      ["pre_tool", "read"],
      ["post_tool", "bash"],
      ["post_tool", "read"],
    ]);
  });

  it("emits session_stop on session.idle", () => {
    const state = emptyMapState();
    const { hooks } = mapEvent({ type: "session.idle", properties: {} }, state);
    expect(hooks).toHaveLength(1);
    expect(hooks[0].kind).toBe("session_stop");
    expect(hooks[0].tool).toBe("");
  });

  it("accumulates usage from step-finish parts", () => {
    const state = emptyMapState();
    const sf = (input: number, output: number, cost: number) => ({
      type: "message.part.updated" as const,
      properties: {
        part: {
          type: "step-finish",
          tokens: { input, output, reasoning: 0, cache: { write: 0, read: 0 } },
          cost,
        },
      },
    });
    mapEvent(sf(100, 20, 0.01), state);
    mapEvent(sf(150, 30, 0.02), state);
    expect(state.usage).toEqual({ in_tokens: 250, out_tokens: 50, cost_usd: 0.03, saw: true });
  });

  it("ignores irrelevant event types without crashing", () => {
    const state = emptyMapState();
    for (const t of [
      "server.connected", "server.heartbeat", "session.created",
      "session.updated", "session.diff", "message.updated", "message.part.delta",
    ]) {
      const { hooks } = mapEvent({ type: t, properties: {} }, state);
      expect(hooks).toEqual([]);
    }
  });

  it("ignores tool parts in pending state", () => {
    const state = emptyMapState();
    const { hooks } = mapEvent(partTool("c1", "bash", "pending"), state);
    expect(hooks).toEqual([]);
  });
});
