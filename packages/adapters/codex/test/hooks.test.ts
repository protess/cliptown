import { describe, it, expect } from "vitest";
import { codexAdapter } from "../src/index.js";

describe("codexAdapter shape", () => {
  it("declares correct id + capabilities", () => {
    expect(codexAdapter.id).toBe("codex");
    expect(codexAdapter.capabilities.block_on_stop).toBe(false);
    expect(codexAdapter.capabilities.inject_context).toBe(false);
    expect(codexAdapter.capabilities.hooks).toEqual([
      "pre_tool", "post_tool", "session_stop", "session_error",
    ]);
    expect(typeof codexAdapter.spawn).toBe("function");
  });
});
