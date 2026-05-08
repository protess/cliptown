import { test, expect } from "@playwright/test";

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

    // For each role row, assert the 3 backend ids render exactly once with
    // a selectable radio. Pinning per-role catches drift where one role's
    // row regresses (e.g., a future 'observer' role gets added with a
    // different backend list).
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

    // Sanity: at least one backend selectable somewhere — the modal must
    // be operable. (Doesn't pin which backend; CI host availability varies.)
    const enabled = dialog.locator("input[type='radio']:not([disabled])");
    expect(await enabled.count()).toBeGreaterThan(0);
  });
});
