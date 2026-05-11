import { describe, it, expect } from "vitest";
import { parseWorkerArgs, substitutePlaceholders, pickAdapter } from "../src/main.js";

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
    expect(a.real).toBe(false); // default
  });

  it("accepts --real for one-shot real-LLM mode", () => {
    const a = parseWorkerArgs([...baseArgs, "--real", "--prompt", "write spec"]);
    expect(a.real).toBe(true);
    expect(a.prompt).toBe("write spec");
    expect(a.mock).toBe(false);
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

describe("substitutePlaceholders", () => {
  it("rewrites __STARTUP_ID__ in top-level string values", () => {
    const out = substitutePlaceholders(
      { task_id: "T1", artifact_path: "workspaces/__STARTUP_ID__/artifacts/T1.md" },
      "s7",
    );
    expect(out).toEqual({
      task_id: "T1",
      artifact_path: "workspaces/s7/artifacts/T1.md",
    });
  });

  it("recurses into nested objects (e.g. mcp args.params)", () => {
    const out = substitutePlaceholders(
      {
        method: "read_assert",
        params: { path: "workspaces/__STARTUP_ID__/spec.md", contains: "Goal" },
      },
      "alpha",
    );
    expect(out).toEqual({
      method: "read_assert",
      params: { path: "workspaces/alpha/spec.md", contains: "Goal" },
    });
  });

  it("leaves non-string scalars and arrays untouched", () => {
    const out = substitutePlaceholders(
      { count: 3, ok: true, tags: ["a", "__STARTUP_ID__"] },
      "s1",
    );
    // Numbers/bools pass through; arrays are opaque (no current fixture
    // uses string arrays as args, so we don't recurse into them).
    expect(out.count).toBe(3);
    expect(out.ok).toBe(true);
    expect(out.tags).toEqual(["a", "__STARTUP_ID__"]);
  });

  it("is a no-op when the placeholder is absent", () => {
    const args = { task_id: "T1", artifact_path: "workspaces/s1/artifacts/T1.md" };
    expect(substitutePlaceholders(args, "s9")).toEqual(args);
  });
});

describe("pickAdapter (M9.10 A2)", () => {
  it("returns claudeCodeAdapter for claude_code", () => {
    const a = pickAdapter("claude_code");
    expect(a.id).toBe("claude_code");
  });

  it("throws not_yet_supported_in_real_mode for codex/opencode", () => {
    expect(() => pickAdapter("codex")).toThrow(/not_yet_supported_in_real_mode/);
    expect(() => pickAdapter("opencode")).toThrow(/not_yet_supported_in_real_mode/);
  });

  it("throws unknown backend for anything else", () => {
    expect(() => pickAdapter("gpt5")).toThrow(/unknown backend/);
  });
});
