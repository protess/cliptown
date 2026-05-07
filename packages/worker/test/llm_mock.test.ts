import { describe, it, expect } from "vitest";
import { createHash } from "node:crypto";
import { LLMMock } from "../src/llm_mock.js";

describe("LLMMock fixture replay", () => {
  it("loads engineer_writes_spec fixture and emits 6 tool_uses + null", () => {
    const m = new LLMMock({ defaultFixture: "engineer_writes_spec" });
    m.setPrompt("ANY-PROMPT");
    expect(m.remaining()).toBe(6);
    expect(m.next()).toMatchObject({ kind: "mcp", tool: "hypothesis_state" });
    expect(m.next()).toMatchObject({ kind: "writeFile", path: "spec.md" });
    expect(m.next()).toMatchObject({ kind: "mcp", tool: "verify" });
    expect(m.next()).toMatchObject({ kind: "mcp", tool: "test_record" });
    expect(m.next()).toMatchObject({ kind: "mcp", tool: "hypothesis_resolve" });
    expect(m.next()).toMatchObject({ kind: "mcp", tool: "task_done" });
    expect(m.next()).toBeNull();
  });

  it("routes prompt by sha256 hash to a named fixture", () => {
    const prompt = "write the cliptown spec";
    const hash = createHash("sha256").update(prompt).digest("hex");
    const m = new LLMMock({ routes: { [hash]: "engineer_writes_spec" } });
    expect(m.setPrompt(prompt)).toBe("engineer_writes_spec");
    expect(m.next()).not.toBeNull();
  });

  it("throws when no route and no default", () => {
    const m = new LLMMock();
    expect(() => m.setPrompt("anything")).toThrow(/no fixture for prompt-hash/);
  });

  it("throws on missing fixture file", () => {
    const m = new LLMMock({ defaultFixture: "this_does_not_exist" });
    expect(() => m.setPrompt("x")).toThrow(/fixture not found/);
  });

  it("loadFixture bypasses hash lookup", () => {
    const m = new LLMMock();
    m.loadFixture("engineer_writes_spec");
    expect(m.loadedFixture()).toBe("engineer_writes_spec");
    expect(m.remaining()).toBe(6);
  });

  it("each next() advances the cursor; remaining() reflects state", () => {
    const m = new LLMMock({ defaultFixture: "engineer_writes_spec" });
    m.setPrompt("x");
    expect(m.remaining()).toBe(6);
    m.next();
    expect(m.remaining()).toBe(5);
    while (m.next() !== null) {
      /* drain */
    }
    expect(m.remaining()).toBe(0);
    expect(m.next()).toBeNull();
  });
});
