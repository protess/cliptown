import { describe, it, expect } from "vitest";
import { parseWorkerArgs } from "../src/main.js";

describe("worker arg parsing", () => {
  const baseArgs = [
    "--world-url", "ws://localhost:8080/ws/worker",
    "--agent-id", "a1",
    "--startup-id", "s1",
    "--secret", "shh",
    "--workspace", "/tmp/ws/s1",
  ];

  it("parses all required args", () => {
    const a = parseWorkerArgs(baseArgs);
    expect(a.worldUrl).toBe("ws://localhost:8080/ws/worker");
    expect(a.agentId).toBe("a1");
    expect(a.startupId).toBe("s1");
    expect(a.secret).toBe("shh");
    expect(a.workspace).toBe("/tmp/ws/s1");
    expect(a.backend).toBe("claude_code"); // default
    expect(a.mock).toBe(false);
    expect(a.fixture).toBeUndefined();
    expect(a.prompt).toBe("");
  });

  it("throws on missing --world-url", () => {
    const args = baseArgs.filter((_, i, arr) => !(arr[i-1] === "--world-url" || arr[i] === "--world-url"));
    expect(() => parseWorkerArgs(args)).toThrow(/missing required arg --world-url/);
  });

  it("throws on missing --agent-id", () => {
    const args = baseArgs.filter((_, i, arr) => !(arr[i-1] === "--agent-id" || arr[i] === "--agent-id"));
    expect(() => parseWorkerArgs(args)).toThrow(/missing required arg --agent-id/);
  });

  it("accepts --mock and --fixture", () => {
    const a = parseWorkerArgs([...baseArgs, "--mock", "--fixture", "engineer_writes_spec", "--prompt", "go"]);
    expect(a.mock).toBe(true);
    expect(a.fixture).toBe("engineer_writes_spec");
    expect(a.prompt).toBe("go");
  });

  it("respects --backend override", () => {
    const a = parseWorkerArgs([...baseArgs, "--backend", "codex"]);
    expect(a.backend).toBe("codex");
  });

  it("rejects unknown args under strict mode", () => {
    expect(() => parseWorkerArgs([...baseArgs, "--unknown", "x"])).toThrow();
  });
});
