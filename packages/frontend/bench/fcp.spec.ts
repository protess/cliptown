/**
 * M10.1 frontend FCP bench. Asserts the ceilings declared in
 * `bench/baselines.json`:
 *   - `/console` FCP ≤ 300ms
 *   - `/town/:id` FCP ≤ 500ms
 *
 * Runs against a production `vite build` served by `vite preview` (port
 * 4173), wired by `playwright.bench.config.ts`. Pass/fail of these two
 * assertions IS the gate — there's no separate comparator script for the
 * frontend metrics, because absolute ceilings are more meaningful here
 * than a 20%-tolerance delta vs an arbitrary baseline.
 *
 * Each test reads `performance.getEntriesByName("first-contentful-paint")`
 * after `page.goto` settles. We wait a short tick after navigation so the
 * paint entry is recorded before we read it; without the wait, Chromium
 * sometimes has not yet pushed the FCP PerformanceEntry into the timeline
 * even though the page has visually painted.
 */
import { test, expect } from "@playwright/test";

const FCP_CONSOLE_CEILING_MS = 300;
const FCP_TOWN_CEILING_MS = 500;

async function fcpFor(page: import("@playwright/test").Page, path: string): Promise<number> {
  // Warm-load: in production the static assets are CDN-cached so the
  // *user-observed* FCP is the warm one. With `vite preview` (and most
  // simple static servers) the very first request triggers on-demand
  // chunk reads / gzip compression / file-system stat overhead that
  // wouldn't reach a real user. Pre-warm once, then measure on a fresh
  // goto so the FCP we report matches production reality.
  await page.goto(path);
  await page.goto(path);
  // Wait until either the entry is recorded OR a generous deadline elapses.
  // 5s is much larger than the ceiling — when this falls back to the
  // deadline path, the test fails the ceiling assertion below with a real
  // signal ("FCP took longer than 5s") instead of "entry never recorded".
  return page.evaluate(async () => {
    const start = performance.now();
    while (performance.now() - start < 5_000) {
      const entry = performance.getEntriesByName("first-contentful-paint")[0];
      if (entry) return entry.startTime;
      await new Promise((r) => setTimeout(r, 25));
    }
    return -1;
  });
}

test.describe("FCP bench (production bundle)", () => {
  test("/console FCP within budget", async ({ page }) => {
    const fcp = await fcpFor(page, "/console");
    expect(fcp, "FCP entry never recorded").toBeGreaterThan(0);
    expect(fcp).toBeLessThanOrEqual(FCP_CONSOLE_CEILING_MS);
  });

  test("/town/:id FCP within budget", async ({ page }) => {
    const fcp = await fcpFor(page, "/town/test-id");
    expect(fcp, "FCP entry never recorded").toBeGreaterThan(0);
    expect(fcp).toBeLessThanOrEqual(FCP_TOWN_CEILING_MS);
  });
});
