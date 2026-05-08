import { test, expect, type Page } from "@playwright/test";

/**
 * Phase 1 ship-gate invariants. Each test in this file lifts one of the
 * 9 spec invariants (docs/superpowers/specs/2026-05-07-cliptown-design.md
 * §11) from a rust-layer proof to a UI-observable proof.
 *
 * docs/superpowers/ship-gate.md tracks the cross-reference between each
 * invariant and its rust-layer test. Phase 0 shipped rust proofs for all
 * 9; Phase 1 lifts them to Playwright UI proofs one by one.
 *
 * Convention: each test is named `§ 11.N — <short claim>` so a failing
 * suite makes it obvious which invariant regressed.
 */

const EXPECTED_BACKENDS = ["claude_code", "codex", "opencode"] as const;
const EXPECTED_ROLES = ["founder", "engineer", "designer"] as const;

// Inject a ConsoleOutbound frame into the live store via the DEV-only test
// hook (store.ts:useConsole). `__cliptownDispatch` is undefined in production
// builds, so this helper only works inside the test environment.
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

// Disconnect the live WS so the test owns the store. Without this, the
// world's periodic snapshots race synthetic dispatches and clobber them.
async function stopWS(page: Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as typeof window & { __cliptownStopWS?: () => void };
    if (!w.__cliptownStopWS) {
      throw new Error("__cliptownStopWS not available — store DEV hook missing");
    }
    w.__cliptownStopWS();
  });
}

test.describe("ship gate § 11", () => {
  // § 11.1 — Multiple startups (≥3) auto-spawn, each claiming a free suite
  // slot; their workers connect via WS; each worker spawns the CLI determined
  // by the agent's `backend` field.
  //
  // Phase 0 swaps "auto-spawn" for "operator-creates-via-modal," but the
  // architectural claim is the same: all 3 adapters reach the operator's UI
  // through the world's BackendCatalog WS push, and each is selectable per
  // agent role. Probe (`probe_all` in crates/world/src/backend_catalog.rs)
  // hard-codes the 3 ids, so this assertion holds regardless of which CLIs
  // happen to be installed on the host (CI may have zero installed; the
  // catalog still reports the 3 ids with `available: false`).
  test("§ 11.1 — three adapters present in NewStartupModal per role", async ({
    page,
  }) => {
    await page.goto("/console");
    await page.getByRole("button", { name: "+ New Startup" }).click();

    const dialog = page.getByRole("dialog", { name: "New startup" });
    await expect(dialog).toBeVisible();

    // The backend catalog may need a tick to arrive over WS — the modal
    // also seeds an empty-catalog hint we explicitly do not want to see.
    await expect(
      dialog.getByText("Catalog empty — try Recheck Backends"),
    ).toHaveCount(0);

    // For each role row, assert the 3 backend ids render exactly once.
    // Pinning per-role catches drift where one role's row regresses (e.g.,
    // a future 'observer' role gets added with a different backend list).
    //
    // We deliberately do NOT assert `:not([disabled])` here. The
    // architectural claim is "all 3 adapter ids reach the UI catalog,"
    // which holds whether the host has zero CLIs installed (CI) or all
    // three (dev laptop). Whether radios are selectable is a downstream
    // availability property, tested by the rust-layer probe_one suite.
    for (const role of EXPECTED_ROLES) {
      const radios = dialog.locator(`input[name="backend-${role}"]`);
      await expect(radios).toHaveCount(EXPECTED_BACKENDS.length);

      for (const id of EXPECTED_BACKENDS) {
        const radio = dialog.locator(
          `input[name="backend-${role}"][value="${id}"]`,
        );
        await expect(radio).toHaveCount(1);
      }
    }
  });

  // § 11.7 — Cross-startup serendipity in the Cafe. When agents from two
  // distinct startups are both in the Cafe, a `proximity_tick` event reaches
  // both workers; if either's CLI emits `speak { kind: "chat" }`, the message
  // is delivered to the other's CLI as `chat_received`. Spec § 11.7.
  //
  // The full routing claim (proximity_tick → chat_received) is rust-layer
  // (`crates/world/tests/e2e_cafe.rs`). This UI proof is narrower: it asserts
  // the rendering surface invariant — when a cross-startup chat arrives in
  // ChatPanel state, the operator sees it visually distinguished from
  // same-startup messages (the `cross-startup tag` polished in PR #7: full
  // opacity hue dot + truncated startup_id span). Without that distinction,
  // cross-startup serendipity collapses into ambient noise.
  //
  // M5+ owns the world emit path for chat/directive frames; until then this
  // test injects synthetic frames via the DEV-only `__cliptownDispatch` hook
  // (store.ts). When M5 lands and the world starts emitting real frames,
  // this test still passes — the rendering contract doesn't change.
  test("§ 11.7 — cross-startup chat in cafe renders the cross tag", async ({
    page,
  }) => {
    const SCOPED_STARTUP = "aaaa1111-aaaa-4111-aaaa-111111111111";
    const OTHER_STARTUP = "bbbb2222-bbbb-4222-bbbb-222222222222";
    const SCOPED_AGENT = "cccc3333-cccc-4333-cccc-333333333333";
    const OTHER_AGENT = "dddd4444-dddd-4444-dddd-444444444444";

    await page.goto("/console");

    // Disconnect the live WS first. The world pushes its own snapshots on
    // any state change (and possibly on a heartbeat), which would race and
    // clobber our synthetic state mid-test. After stopWS, the store is ours.
    await stopWS(page);

    // Seed an operator avatar so ChatPanel's scopedStartup resolves to
    // SCOPED_STARTUP and scopedRoom resolves to "cafe". Without scope, every
    // message renders as same-startup (cross is always false), which would
    // mask the very invariant we're testing.
    await dispatch(page, {
      type: "world_view_snapshot",
      snapshot: {
        avatars: [
          {
            agent_id: "__operator__",
            startup_id: SCOPED_STARTUP,
            role: "operator",
            backend: "operator",
            current_pos: [20, 5],
            target_pos: null,
            room_id: "cafe",
            status: "idle",
          },
        ],
      },
    });

    // Open chat panel.
    await page.getByRole("button", { name: "Open chat" }).click();
    const chat = page.getByRole("complementary", { name: "Chat" });
    await expect(chat).toBeVisible();

    // Inject one same-startup chat (should render WITHOUT the cross tag) and
    // one cross-startup chat (should render WITH it).
    await dispatch(page, {
      type: "chat",
      id: "msg-same",
      ts: Date.now(),
      startup_id: SCOPED_STARTUP,
      room_id: "cafe",
      author_id: SCOPED_AGENT,
      body: "same-startup hello",
    });
    await dispatch(page, {
      type: "chat",
      id: "msg-cross",
      ts: Date.now() + 1,
      startup_id: OTHER_STARTUP,
      room_id: "cafe",
      author_id: OTHER_AGENT,
      body: "cross-startup hello",
    });

    // Both bodies render — confirms the room scope (cafe) lets both through.
    await expect(chat.getByText("same-startup hello")).toBeVisible();
    await expect(chat.getByText("cross-startup hello")).toBeVisible();

    // Cross-startup tag: a span carrying title=OTHER_STARTUP, rendered ONLY
    // for the cross bubble. ChatPanel.tsx:230-236 owns this surface.
    const crossTag = chat.locator(`span[title="${OTHER_STARTUP}"]`);
    await expect(crossTag).toHaveCount(1);
    await expect(crossTag).toHaveText(OTHER_STARTUP.slice(0, 6));

    // Same-startup bubble must NOT have a cross tag for the scoped startup.
    const sameTag = chat.locator(`span[title="${SCOPED_STARTUP}"]`);
    await expect(sameTag).toHaveCount(0);

    // Author UUIDs are also truncated (covered by PR #7), but assert here so
    // a regression in either AuthorId or the ChatPanel structure trips the
    // invariant test.
    await expect(
      chat.locator(`code[title="${SCOPED_AGENT}"]`),
    ).toHaveText(SCOPED_AGENT.slice(0, 6));
    await expect(
      chat.locator(`code[title="${OTHER_AGENT}"]`),
    ).toHaveText(OTHER_AGENT.slice(0, 6));
  });
});
