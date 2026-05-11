/**
 * M10.1 frontend FCP bench config — separate from the e2e config so the
 * bench runs against a production `vite build` (served via `vite preview`)
 * instead of the dev server, which would over-report FCP with on-demand
 * module compilation and under-report it with warm caches.
 *
 * Discovers tests under `./bench` (not `./e2e`). One file lives there
 * today: `fcp.spec.ts`, which asserts the ceilings from
 * `bench/baselines.json` (300ms /console, 500ms /town/:id).
 *
 * Invoke via `pnpm -F @cliptown/frontend bench:fcp`. The script builds
 * the frontend first; this config spins up `vite preview` on port 4173
 * (vite's default) so the bench tests target a real production bundle.
 *
 * The world server is NOT brought up here — `/console` and `/town/:id`
 * paint their initial shell (TopBar wordmark, Back link, etc.) before
 * the WebSocket handshake, so FCP is observable without backend data.
 * Skipping the cargo run shaves ~20s off the cold-start path.
 */
import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./bench",
  // Single bench file with 2 tests, both serialized for stable FCP numbers.
  fullyParallel: false,
  workers: 1,
  // FCP is the assertion; we deliberately don't retry — flakes here are
  // information about prod-bundle timing variance, not noise to suppress.
  retries: 0,
  timeout: 60_000,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:4173",
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: [
    {
      command: "pnpm preview --port 4173 --strictPort",
      url: "http://127.0.0.1:4173",
      reuseExistingServer: !process.env.CI,
      timeout: 60_000,
    },
  ],
});
