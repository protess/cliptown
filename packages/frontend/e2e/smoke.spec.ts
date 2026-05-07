import { test, expect } from "@playwright/test";

test.describe("ship gate smoke", () => {
  test("/ redirects to /console and renders the wordmark", async ({ page }) => {
    await page.goto("/");
    await page.waitForURL(/\/console$/);
    await expect(page.locator("text=cliptown")).toBeVisible();
  });

  test("/console renders the wordmark", async ({ page }) => {
    await page.goto("/console");
    // The TopBar wordmark always renders — independent of WS state, system
    // events, or selected startup. Asserting on `code` previously timed out on
    // a fresh page since /console only renders <code> when an event/startup
    // is present.
    await expect(page.locator("text=cliptown").first()).toBeVisible();
  });

  test("/town/:id renders the back link", async ({ page }) => {
    await page.goto("/town/test-id");
    await expect(page.locator("text=Back")).toBeVisible();
  });
});
