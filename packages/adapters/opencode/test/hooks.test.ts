import { describe, it, expect } from "vitest";
import { opencodeAdapter } from "../src/index.js";

describe("opencodeAdapter shape", () => {
  it("declares correct id + capabilities", () => {
    expect(opencodeAdapter.id).toBe("opencode");
    expect(opencodeAdapter.capabilities.inject_context).toBe(true);
    expect(opencodeAdapter.capabilities.block_on_stop).toBe(false);
    expect(opencodeAdapter.capabilities.hooks).toEqual([
      "pre_tool", "post_tool", "session_stop", "session_error",
    ]);
    expect(typeof opencodeAdapter.spawn).toBe("function");
  });
});
