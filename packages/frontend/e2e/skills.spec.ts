import { test, expect, type Page } from "@playwright/test";

/**
 * P2.2 — SkillsPanel UI tests.
 *
 * Uses the same `__cliptownDispatch` / `__cliptownStopWS` DEV-only hooks as
 * ship-gate.spec.ts to inject synthetic ConsoleOutbound frames and own the
 * store deterministically.
 */

async function dispatch(page: Page, msg: unknown): Promise<void> {
  await page.evaluate((m) => {
    const w = window as typeof window & {
      __cliptownDispatch?: (msg: unknown) => void;
    };
    if (!w.__cliptownDispatch) {
      throw new Error(
        "__cliptownDispatch not available — store DEV hook missing",
      );
    }
    w.__cliptownDispatch(m);
  }, msg);
}

async function stopWS(page: Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as typeof window & { __cliptownStopWS?: () => void };
    if (!w.__cliptownStopWS) {
      throw new Error("__cliptownStopWS not available — store DEV hook missing");
    }
    w.__cliptownStopWS();
  });
}

test.describe("§ P2.2 skills panel", () => {
  const STARTUP = "sk11aaaa-sk11-4aaa-sk11-aaaaaaaask11";
  const AGENT_FOUNDER = "sk11f000-0000-4000-0000-000000000001";
  const AGENT_ENGINEER = "sk11e000-0000-4000-0000-000000000002";
  const SKILL_A = "skill-aaaa-0001";
  const SKILL_B = "skill-bbbb-0002";

  test("renders empty state when no startup is possessed", async ({ page }) => {
    await page.goto("/console");
    await stopWS(page);

    // No __operator__ avatar → possessedStartupId is null.
    const panel = page.getByTestId("skills-panel");
    await expect(panel).toBeVisible();
    await expect(panel).toContainText("Possess a startup to see its skills.");
  });

  test("renders skills_snapshot and reflects skill_changed broadcasts", async ({
    page,
  }) => {
    await page.goto("/console");
    await stopWS(page);

    // Seed world: operator possesses STARTUP, two agents present.
    await dispatch(page, {
      type: "world_view_snapshot",
      snapshot: {
        startups: [
          { id: STARTUP, name: "skill-test-co", budget_cap_usd: 100, budget_spent_usd: 0, last_event_ts: 1 },
        ],
        avatars: [
          {
            agent_id: "__operator__",
            startup_id: STARTUP,
            role: "operator",
            backend: "operator",
            current_pos: [0, 0],
            target_pos: null,
            room_id: "suite_1",
            status: "idle",
          },
          {
            agent_id: AGENT_FOUNDER,
            startup_id: STARTUP,
            role: "founder",
            backend: "claude_code",
            current_pos: [0, 0],
            target_pos: null,
            room_id: "suite_1",
            status: "idle",
          },
          {
            agent_id: AGENT_ENGINEER,
            startup_id: STARTUP,
            role: "engineer",
            backend: "claude_code",
            current_pos: [0, 0],
            target_pos: null,
            room_id: "suite_1",
            status: "idle",
          },
        ],
        tasks: [],
      },
    });

    // Inject skills_snapshot: two skills, skill-A attached to AGENT_FOUNDER.
    await dispatch(page, {
      type: "skills_snapshot",
      v: 1,
      startups: {
        [STARTUP]: [
          {
            id: SKILL_A,
            name: "alpha-skill",
            len: 512,
            updated_at: 1_000_000,
            attachments: [AGENT_FOUNDER],
          },
          {
            id: SKILL_B,
            name: "beta-skill",
            len: 1024,
            updated_at: 1_000_001,
            attachments: [],
          },
        ],
      },
    });

    const panel = page.getByTestId("skills-panel");
    await expect(panel).toBeVisible();

    // Both skills render (sorted alphabetically: alpha before beta).
    const rowA = page.getByTestId("skill-row-alpha-skill");
    const rowB = page.getByTestId("skill-row-beta-skill");
    await expect(rowA).toBeVisible();
    await expect(rowB).toBeVisible();

    // Byte counts render.
    await expect(rowA).toContainText("512 bytes");
    await expect(rowB).toContainText("1024 bytes");

    // skill-A has AGENT_FOUNDER attached → chip renders.
    const detachChip = page.getByTestId(`skill-detach-alpha-skill-${AGENT_FOUNDER}`);
    await expect(detachChip).toBeVisible();
    await expect(detachChip).toContainText("founder");

    // skill-B has no attachments → "no attachments" text.
    await expect(rowB).toContainText("no attachments");

    // skill-B's "Attach to…" select should list both agents (neither attached).
    const attachSelect = page.getByTestId("skill-attach-beta-skill");
    await expect(attachSelect).toBeVisible();

    // ── skill_changed: upsert a new skill ──
    await dispatch(page, {
      type: "skill_changed",
      v: 1,
      startup_id: STARTUP,
      kind: "upsert",
      skill_id: "skill-cccc-0003",
      agent_id: null,
      skill: {
        id: "skill-cccc-0003",
        name: "gamma-skill",
        len: 256,
        updated_at: 1_000_002,
        attachments: [],
      },
    });
    await expect(page.getByTestId("skill-row-gamma-skill")).toBeVisible();

    // ── skill_changed: attach AGENT_ENGINEER to skill-B ──
    await dispatch(page, {
      type: "skill_changed",
      v: 1,
      startup_id: STARTUP,
      kind: "attach",
      skill_id: SKILL_B,
      agent_id: AGENT_ENGINEER,
      skill: null,
    });
    // Engineer chip now shows on beta-skill row.
    await expect(
      page.getByTestId(`skill-detach-beta-skill-${AGENT_ENGINEER}`),
    ).toBeVisible();

    // ── skill_changed: detach AGENT_FOUNDER from skill-A ──
    await dispatch(page, {
      type: "skill_changed",
      v: 1,
      startup_id: STARTUP,
      kind: "detach",
      skill_id: SKILL_A,
      agent_id: AGENT_FOUNDER,
      skill: null,
    });
    // Founder chip gone; "no attachments" appears.
    await expect(
      page.getByTestId(`skill-detach-alpha-skill-${AGENT_FOUNDER}`),
    ).toHaveCount(0);
    await expect(rowA).toContainText("no attachments");

    // ── skill_changed: delete gamma-skill ──
    await dispatch(page, {
      type: "skill_changed",
      v: 1,
      startup_id: STARTUP,
      kind: "delete",
      skill_id: "skill-cccc-0003",
      agent_id: null,
      skill: null,
    });
    await expect(page.getByTestId("skill-row-gamma-skill")).toHaveCount(0);
  });
});
