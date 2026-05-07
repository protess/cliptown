import { test, expect } from "@playwright/test";

test.describe("ship gate smoke", () => {
  test("/ redirects to /console and renders the wordmark", async ({ page }) => {
    await page.goto("/");
    await page.waitForURL(/\/console$/);
    await expect(page.locator("text=cliptown")).toBeVisible();
  });

  test("/console shows status indicator", async ({ page }) => {
    await page.goto("/console");
    // The status text "connecting" or "open" or "closed" should be present.
    const status = page.locator("code").first();
    await expect(status).toBeVisible({ timeout: 10_000 });
  });

  test("/town/:id renders the back link", async ({ page }) => {
    await page.goto("/town/test-id");
    await expect(page.locator("text=Back")).toBeVisible();
  });
});
