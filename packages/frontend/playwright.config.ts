import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  fullyParallel: false, // single Vite dev server
  workers: 1,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:5173",
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  // Two servers: the rust world (MUST be up before vite, since vite proxies
  // /ws and /api to :8080) and the vite dev server. `reuseExistingServer`
  // keeps the local dev experience nice — if you already have `pnpm dev`
  // running at the repo root, playwright reuses both processes.
  webServer: [
    {
      command: "cargo run -p cliptown-world",
      cwd: "../..",
      url: "http://127.0.0.1:8080/health",
      reuseExistingServer: !process.env.CI,
      timeout: 120_000,
    },
    {
      command: "pnpm dev",
      url: "http://127.0.0.1:5173",
      reuseExistingServer: !process.env.CI,
      timeout: 120_000,
    },
  ],
});
