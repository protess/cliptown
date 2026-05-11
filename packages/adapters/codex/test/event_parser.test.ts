// packages/adapters/codex/test/event_parser.test.ts
import { describe, it, expect } from "vitest";
import { emptyState, parseChunk, finalize } from "../src/event_parser.js";

describe("codex event_parser", () => {
  it("emits pre_tool then post_tool for a command_execution item", () => {
    const state = emptyState();
    const lines = [
      `{"type":"thread.started","thread_id":"t1"}`,
      `{"type":"turn.started"}`,
      `{"type":"item.started","item":{"id":"item_0","type":"command_execution","command":"/bin/zsh -lc ls","aggregated_output":"","exit_code":null,"status":"in_progress"}}`,
      `{"type":"item.completed","item":{"id":"item_0","type":"command_execution","command":"/bin/zsh -lc ls","aggregated_output":"out\\n","exit_code":0,"status":"completed"}}`,
      `{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"done"}}`,
      `{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":50,"output_tokens":20}}`,
      "",
    ].join("\n");
    const { hooks } = parseChunk(lines, state);
    expect(hooks.map((h) => [h.kind, h.tool])).toEqual([
      ["pre_tool", "shell"],
      ["post_tool", "shell"],
    ]);
    expect(hooks[0].payload).toMatchObject({ type: "command_execution", status: "in_progress" });
    expect(hooks[1].payload).toMatchObject({ type: "command_execution", status: "completed", exit_code: 0 });
    expect(hooks[0].seq).toBe(1);
    expect(hooks[1].seq).toBe(2);
    expect(state.usage).toEqual({ in_tokens: 150, out_tokens: 20, saw: true });
  });

  it("emits pre_tool/post_tool for mcp_tool_call using item.tool field", () => {
    const state = emptyState();
    const lines = [
      `{"type":"item.started","item":{"id":"i0","type":"mcp_tool_call","server":"cliptown","tool":"mcp__cliptown__task_done","status":"in_progress"}}`,
      `{"type":"item.completed","item":{"id":"i0","type":"mcp_tool_call","server":"cliptown","tool":"mcp__cliptown__task_done","result":{"ok":true},"status":"completed"}}`,
      "",
    ].join("\n");
    const { hooks } = parseChunk(lines, state);
    expect(hooks.map((h) => h.kind)).toEqual(["pre_tool", "post_tool"]);
    expect(hooks[0].tool).toBe("mcp__cliptown__task_done");
    expect(hooks[1].tool).toBe("mcp__cliptown__task_done");
  });

  it("falls back to 'mcp' when mcp_tool_call lacks a tool field", () => {
    const state = emptyState();
    const line = `{"type":"item.started","item":{"id":"i0","type":"mcp_tool_call","status":"in_progress"}}` + "\n";
    const { hooks } = parseChunk(line, state);
    expect(hooks[0].tool).toBe("mcp");
  });

  it("skips agent_message and turn boundaries", () => {
    const state = emptyState();
    const lines = [
      `{"type":"thread.started","thread_id":"t1"}`,
      `{"type":"item.completed","item":{"id":"i9","type":"agent_message","text":"hi"}}`,
      `{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}`,
      "",
    ].join("\n");
    const { hooks } = parseChunk(lines, state);
    expect(hooks).toEqual([]);
  });

  it("handles a chunk split mid-line via carry buffer", () => {
    const state = emptyState();
    const first = `{"type":"item.started","item":{"id":"i0","ty`;
    const second = `pe":"command_execution","command":"x","aggregated_output":"","exit_code":null,"status":"in_progress"}}\n`;
    const a = parseChunk(first, state);
    const b = parseChunk(second, state);
    expect(a.hooks).toEqual([]);
    expect(b.hooks).toHaveLength(1);
    expect(b.hooks[0].kind).toBe("pre_tool");
    expect(b.hooks[0].tool).toBe("shell");
  });

  it("silently skips malformed JSON lines", () => {
    const state = emptyState();
    const lines = [
      `not json`,
      `{"type":"item.started","item":{"id":"i0","type":"command_execution","status":"in_progress"}}`,
      "",
    ].join("\n");
    const { hooks } = parseChunk(lines, state);
    expect(hooks).toHaveLength(1);
  });

  it("finalize emits session_stop on exit 0", () => {
    const state = emptyState();
    parseChunk(`{"type":"turn.completed","usage":{"input_tokens":7,"output_tokens":3}}\n`, state);
    const { hooks } = finalize(state, { exit_code: 0, signal: undefined, stderr_tail: "" });
    expect(hooks).toHaveLength(1);
    expect(hooks[0].kind).toBe("session_stop");
    expect(hooks[0].payload).toMatchObject({ exit_code: 0 });
  });

  it("finalize emits session_error on non-zero exit and attaches stderr_tail", () => {
    const state = emptyState();
    const { hooks } = finalize(state, { exit_code: 1, signal: undefined, stderr_tail: "boom" });
    expect(hooks).toHaveLength(1);
    expect(hooks[0].kind).toBe("session_error");
    expect(hooks[0].payload).toMatchObject({ exit_code: 1, stderr_tail: "boom" });
  });

  it("finalize flushes any pending partial line", () => {
    const state = emptyState();
    parseChunk(
      `{"type":"item.completed","item":{"id":"i0","type":"command_execution","exit_code":0,"status":"completed"}}`,
      state,
    );
    // No trailing newline; line is still in carry until finalize.
    const { hooks } = finalize(state, { exit_code: 0, signal: undefined, stderr_tail: "" });
    // First the flushed post_tool, then session_stop.
    expect(hooks.map((h) => h.kind)).toEqual(["post_tool", "session_stop"]);
  });
});
