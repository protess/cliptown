import { describe, it, expect } from "vitest";
import { opencodeAdapter } from "../src/index.js";

describe("opencodeAdapter shape", () => {
  it("declares correct id + capabilities", () => {
    expect(opencodeAdapter.id).toBe("opencode");
    expect(opencodeAdapter.capabilities.inject_context).toBe(true);
    expect(opencodeAdapter.capabilities.block_on_stop).toBe(false);
    expect(opencodeAdapter.capabilities.hooks).toContain("session_stop");
    expect(opencodeAdapter.capabilities.hooks).toContain("session_error");
    expect(typeof opencodeAdapter.spawn).toBe("function");
  });
});
