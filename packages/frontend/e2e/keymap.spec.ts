import { test, expect, type Page } from "@playwright/test";

/**
 * Pin the global keymap's input-suppression behavior. The audit (see
 * keymap.ts:38 isEditable) covers INPUT/TEXTAREA/SELECT/contenteditable,
 * which today catches every input surface in the app. If a future surface
 * uses a non-native input (e.g., a `role="textbox"` div), this test fails
 * and forces an isEditable update at the same time.
 *
 * Letters tested: t (→/town), p (possess toggle), c (chat open), j/k
 * (sidebar cycle). All must be typeable inside an input without firing the
 * corresponding global command.
 */

const COMMAND_LETTERS = ["t", "p", "c", "j", "k"] as const;

async function installAudit(page: Page): Promise<void> {
  await page.evaluate(() => {
    interface AuditState {
      events: string[];
      navigations: Array<{ to: unknown; from: string }>;
    }
    const w = window as typeof window & { __keymapAudit?: AuditState };
    w.__keymapAudit = { events: [], navigations: [] };
    const origDispatch = window.dispatchEvent.bind(window);
    window.dispatchEvent = (ev: Event) => {
      if (ev.type.startsWith("cliptown:")) {
        w.__keymapAudit!.events.push(ev.type);
      }
      return origDispatch(ev);
    };
    const origPush = history.pushState.bind(history);
    history.pushState = (...args: Parameters<typeof history.pushState>) => {
      w.__keymapAudit!.navigations.push({
        to: args[2],
        from: location.pathname,
      });
      return origPush(...args);
    };
  });
}

async function readAudit(page: Page): Promise<{
  events: string[];
  navigations: Array<{ to: unknown; from: string }>;
}> {
  return page.evaluate(() => {
    const w = window as typeof window & {
      __keymapAudit?: { events: string[]; navigations: unknown[] };
    };
    return {
      events: w.__keymapAudit?.events ?? [],
      navigations:
        (w.__keymapAudit?.navigations as Array<{
          to: unknown;
          from: string;
        }>) ?? [],
    };
  });
}

test.describe("keymap input suppression", () => {
  test("typing command letters in NewStartupModal inputs does not fire global commands", async ({
    page,
  }) => {
    await page.goto("/console");
    await installAudit(page);

    await page.getByRole("button", { name: "+ New Startup" }).click();
    await expect(
      page.getByRole("dialog", { name: "New startup" }),
    ).toBeVisible();

    // Name input — clear, then type each command letter individually so the
    // global keydown listener sees a separate key event for each.
    const name = page.getByRole("textbox").first();
    await name.click();
    await name.fill("");
    for (const k of COMMAND_LETTERS) {
      await page.keyboard.press(k);
    }
    await expect(name).toHaveValue(COMMAND_LETTERS.join(""));

    // Goal textarea — same drill. Use the existing prefill, just append.
    const goal = page.locator("textarea");
    await goal.click();
    const before = (await goal.inputValue()) ?? "";
    for (const k of COMMAND_LETTERS) {
      await page.keyboard.press(k);
    }
    await expect(goal).toHaveValue(before + COMMAND_LETTERS.join(""));

    const audit = await readAudit(page);
    expect(audit.events).toEqual([]);
    expect(audit.navigations).toEqual([]);
    expect(page.url()).toMatch(/\/console$/);
  });

  test("Escape inside an input dispatches cliptown:dismiss and closes the modal", async ({
    page,
  }) => {
    await page.goto("/console");
    await installAudit(page);

    await page.getByRole("button", { name: "+ New Startup" }).click();
    const dialog = page.getByRole("dialog", { name: "New startup" });
    await expect(dialog).toBeVisible();

    await page.locator("textarea").click();
    await page.keyboard.press("Escape");

    await expect(dialog).toBeHidden();
    const audit = await readAudit(page);
    expect(audit.events).toEqual(["cliptown:dismiss"]);
  });
});
