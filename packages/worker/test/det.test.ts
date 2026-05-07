import { testCtx } from "../src/det";
import { describe, it, expect } from "vitest";
describe("det", () => {
  it("is reproducible with the same seed", () => {
    const a = testCtx(100, 7); const b = testCtx(100, 7);
    expect(a.random.nextU32()).toBe(b.random.nextU32());
    expect(a.uuid.new()).toBe(b.uuid.new());
  });
});
