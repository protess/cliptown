import { test, expect } from "@playwright/test";

// Phase 0: skipped by default — runs in M10.1 CI when chromium is installed.
// Ship-gate target: /console FCP ≤ 300ms, /town/:id FCP ≤ 500ms.
test.describe.skip("FCP benches", () => {
  test("/console FCP within budget", async ({ page }) => {
    await page.goto("/console");
    const fcp = await page.evaluate(() => {
      const e = performance.getEntriesByName("first-contentful-paint")[0];
      return e?.startTime ?? -1;
    });
    expect(fcp).toBeGreaterThan(0);
    expect(fcp).toBeLessThanOrEqual(300);
  });

  test("/town/:id FCP within budget", async ({ page }) => {
    await page.goto("/town/test-id");
    const fcp = await page.evaluate(() => {
      const e = performance.getEntriesByName("first-contentful-paint")[0];
      return e?.startTime ?? -1;
    });
    expect(fcp).toBeGreaterThan(0);
    expect(fcp).toBeLessThanOrEqual(500);
  });
});
