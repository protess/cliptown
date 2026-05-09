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

  // § 11.8 — Multi-tenant isolation. A directive sent inside one startup's
  // suite is never delivered to any agent of any other startup, regardless
  // of timing, room transitions, or backend type. Spec § 11.8.
  //
  // The full delivery claim (worker-side routing) lives in the rust-layer
  // `crates/world/tests/e2e_isolation.rs`. The UI proof is the operator's
  // observable side: when the operator selects startup A in the sidebar,
  // MainHeader counts and the Kanban only show A's data — never B's. If
  // the per-startup filters in MainHeader (`a.startup_id === s.id`) or
  // Kanban (`t.startup_id === startupId`) regressed and pulled in
  // foreign-startup state, the operator would see ghost agents/tasks from
  // other tenants. This test pins that contract.
  test("§ 11.8 — selecting a startup partitions MainHeader and Kanban to that startup only", async ({
    page,
  }) => {
    const STARTUP_A = "aaaa1111-aaaa-4111-aaaa-111111111111";
    const STARTUP_B = "bbbb2222-bbbb-4222-bbbb-222222222222";

    await page.goto("/console");
    await stopWS(page);

    // Two startups, each with its own agents and distinctive task titles.
    // Distinctive titles let us assert both directions: A's titles must NOT
    // appear when B is selected, and vice-versa.
    await dispatch(page, {
      type: "world_view_snapshot",
      snapshot: {
        startups: [
          { id: STARTUP_A, name: "alpha-iso", budget_cap_usd: 100, budget_spent_usd: 0, last_event_ts: 1 },
          { id: STARTUP_B, name: "beta-iso", budget_cap_usd: 50, budget_spent_usd: 0, last_event_ts: 2 },
        ],
        avatars: [
          { agent_id: "a1aaaaaa-aaaa-4111-aaaa-aaaaaaaaaaaa", startup_id: STARTUP_A, role: "founder", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
          { agent_id: "a2aaaaaa-aaaa-4222-aaaa-aaaaaaaaaaaa", startup_id: STARTUP_A, role: "engineer", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
          { agent_id: "a3aaaaaa-aaaa-4333-aaaa-aaaaaaaaaaaa", startup_id: STARTUP_A, role: "designer", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
          { agent_id: "b1bbbbbb-bbbb-4111-bbbb-bbbbbbbbbbbb", startup_id: STARTUP_B, role: "founder", backend: "codex", current_pos: [0, 0], target_pos: null, room_id: "suite_2", status: "idle" },
          { agent_id: "b2bbbbbb-bbbb-4222-bbbb-bbbbbbbbbbbb", startup_id: STARTUP_B, role: "engineer", backend: "codex", current_pos: [0, 0], target_pos: null, room_id: "suite_2", status: "idle" },
        ],
        tasks: [
          { id: "task-a-001", startup_id: STARTUP_A, title: "TASK-ALPHA-WRITE-SPEC", status: "queued", assignee_agent_id: null, required_room: null },
          { id: "task-a-002", startup_id: STARTUP_A, title: "TASK-ALPHA-RESEARCH", status: "queued", assignee_agent_id: null, required_room: null },
          { id: "task-b-001", startup_id: STARTUP_B, title: "TASK-BETA-PROTOTYPE", status: "queued", assignee_agent_id: null, required_room: null },
          { id: "task-b-002", startup_id: STARTUP_B, title: "TASK-BETA-DESIGN", status: "queued", assignee_agent_id: null, required_room: null },
          { id: "task-b-003", startup_id: STARTUP_B, title: "TASK-BETA-REVIEW", status: "queued", assignee_agent_id: null, required_room: null },
        ],
      },
    });

    const sidebar = page.getByRole("complementary", { name: "startups" });
    const main = page.locator("main");

    // Both startups are listed in the sidebar — that's expected (the operator
    // sees all tenants from a god view). Isolation is about which DATA
    // the per-startup main pane shows, not which startups appear in the
    // sidebar.
    await expect(sidebar.locator(`[data-startup-id="${STARTUP_A}"]`)).toBeVisible();
    await expect(sidebar.locator(`[data-startup-id="${STARTUP_B}"]`)).toBeVisible();

    // Helper to assert MainHeader's stat tile (label + value) — Stat in
    // MainHeader.tsx renders <div>{value}</div><div>{label}</div> as
    // siblings inside a wrapper.
    const statValue = async (label: string): Promise<string> => {
      return main
        .locator("div")
        .filter({ has: page.locator(`> div:text-is("${label}")`) })
        .first()
        .locator("> div")
        .first()
        .innerText();
    };

    // Select startup A.
    await sidebar.locator(`[data-startup-id="${STARTUP_A}"]`).click();

    await expect(main.getByText("alpha-iso", { exact: true })).toBeVisible();
    expect(await statValue("agents")).toBe("3");
    expect(await statValue("tasks")).toBe("2");
    // Kanban shows A's titles, not B's.
    await expect(main.getByText("TASK-ALPHA-WRITE-SPEC")).toBeVisible();
    await expect(main.getByText("TASK-ALPHA-RESEARCH")).toBeVisible();
    await expect(main.getByText("TASK-BETA-PROTOTYPE")).toHaveCount(0);
    await expect(main.getByText("TASK-BETA-DESIGN")).toHaveCount(0);
    await expect(main.getByText("TASK-BETA-REVIEW")).toHaveCount(0);

    // Select startup B.
    await sidebar.locator(`[data-startup-id="${STARTUP_B}"]`).click();

    await expect(main.getByText("beta-iso", { exact: true })).toBeVisible();
    expect(await statValue("agents")).toBe("2");
    expect(await statValue("tasks")).toBe("3");
    // Kanban now shows B's titles, none of A's.
    await expect(main.getByText("TASK-BETA-PROTOTYPE")).toBeVisible();
    await expect(main.getByText("TASK-BETA-DESIGN")).toBeVisible();
    await expect(main.getByText("TASK-BETA-REVIEW")).toBeVisible();
    await expect(main.getByText("TASK-ALPHA-WRITE-SPEC")).toHaveCount(0);
    await expect(main.getByText("TASK-ALPHA-RESEARCH")).toHaveCount(0);
  });

  // § 11.3 — A task with required_room: library causes the assignee's avatar
  // to walk from its suite to the Library along the A* path. Spec § 11.3.
  //
  // The full A* + door routing claim lives in the rust-layer
  // `crates/world/tests/movement.rs`. This UI proof asserts the operator's
  // visual side: when the world publishes a snapshot with a new `target_pos`
  // for an agent, the Pixi sprite animates toward that target. Without that
  // animation, the operator can't see the walk happen and the spec's "watch
  // it walk" guarantee is invisible.
  //
  // Pixi sprites aren't in the DOM, so the assertion uses the dev-only
  // `__cliptownInspectAvatarSprite` hook (PixiStage.tsx) which returns the
  // live screen-pixel position of an agent's sprite. The hook tree-shakes
  // away in production builds.
  test("§ 11.3 — sprite animates toward target_pos when world publishes a path step", async ({
    page,
  }) => {
    const STARTUP = "abcd1234-abcd-4123-abcd-123456789012";
    const AGENT = "engr1111-engr-4111-engr-111111111111";
    const TILE = 20;
    const HALF = TILE / 2;
    const initialTile: [number, number] = [3, 3];
    const targetTile: [number, number] = [15, 10];

    await page.goto(`/town/${STARTUP}`);
    await stopWS(page);

    // Pixi.Application.init is async — wait for the sprite-inspector hook
    // and the avatar layer to come online. The hook is a `function` once
    // PixiStage's effect mounts AND the DEV-only block has wired it up.
    await page.waitForFunction(
      () =>
        typeof (window as unknown as {
          __cliptownInspectAvatarSprite?: unknown;
        }).__cliptownInspectAvatarSprite === "function",
    );

    // Place the agent in suite_1 with no target — sprite should mount at
    // the centered tile pixel coords.
    await dispatch(page, {
      type: "world_view_snapshot",
      snapshot: {
        avatars: [
          {
            agent_id: AGENT,
            startup_id: STARTUP,
            role: "engineer",
            backend: "claude_code",
            current_pos: initialTile,
            target_pos: null,
            room_id: "suite_1",
            status: "idle",
          },
        ],
      },
    });

    // Wait for the sprite to mount and snap to the initial pixel position.
    const initialPx = {
      x: initialTile[0] * TILE + HALF,
      y: initialTile[1] * TILE + HALF,
    };
    await page.waitForFunction(
      ({ id, want }) => {
        const w = window as unknown as {
          __cliptownInspectAvatarSprite: (
            id: string,
          ) => { x: number; y: number; alpha: number } | null;
        };
        const s = w.__cliptownInspectAvatarSprite(id);
        return (
          s != null &&
          Math.abs(s.x - want.x) < 2 &&
          Math.abs(s.y - want.y) < 2
        );
      },
      { id: AGENT, want: initialPx },
    );

    // World publishes a movement step — target_pos in the library. The Pixi
    // ticker (Avatars.tsx interpolatePosition over TICK_DURATION_MS=1000ms)
    // lerps the sprite from current toward target.
    await dispatch(page, {
      type: "world_view_snapshot",
      snapshot: {
        avatars: [
          {
            agent_id: AGENT,
            startup_id: STARTUP,
            role: "engineer",
            backend: "claude_code",
            current_pos: initialTile,
            target_pos: targetTile,
            room_id: "suite_1",
            status: "working",
          },
        ],
      },
    });

    // Sprite must measurably advance past the initial x within the
    // animation window. This catches the "sprite doesn't animate" failure
    // mode (e.g., updateAvatarTargets regresses, or the ticker stalls).
    await page.waitForFunction(
      ({ id, beyondX }) => {
        const w = window as unknown as {
          __cliptownInspectAvatarSprite: (
            id: string,
          ) => { x: number; y: number; alpha: number } | null;
        };
        const s = w.__cliptownInspectAvatarSprite(id);
        return s != null && s.x > beyondX;
      },
      { id: AGENT, beyondX: initialPx.x + TILE }, // moved at least one full tile
      { timeout: 3_000 },
    );

    // After the full TICK_DURATION_MS the sprite should be at (or within
    // 2px of) the target. Generous tolerance because the lerp is clamped
    // but headless ticker timing varies.
    const targetPx = {
      x: targetTile[0] * TILE + HALF,
      y: targetTile[1] * TILE + HALF,
    };
    await page.waitForFunction(
      ({ id, want }) => {
        const w = window as unknown as {
          __cliptownInspectAvatarSprite: (
            id: string,
          ) => { x: number; y: number; alpha: number } | null;
        };
        const s = w.__cliptownInspectAvatarSprite(id);
        return (
          s != null &&
          Math.abs(s.x - want.x) < 2 &&
          Math.abs(s.y - want.y) < 2
        );
      },
      { id: AGENT, want: targetPx },
      { timeout: 3_000 },
    );
  });

  // § 11.2 — An operator directive sent to a founder produces (a) a Directive
  // frame visible in the operator's chat panel, (b) a new subtask appearing
  // in the founder's startup's Kanban as "queued", and (c) the task
  // transitioning to "in progress" once the scheduler assigns it to the
  // engineer. Spec § 11.2.
  //
  // The full chain (cmd_console::OperatorDirective → mcp_dispatch::handle_subtask_create
  // → scheduler::tick → task_assigned) lives in the rust-layer
  // `crates/world/tests/e2e_directive_chain.rs`. Until M5+ wires a real-WS
  // end-to-end fixture in CI, this UI proof asserts each observable
  // transition by synthesizing the ConsoleOutbound frames the world would
  // emit at each step:
  //   1. Directive frame after `cmd_console::OperatorDirective` succeeds
  //      (M5 emit path; commit 553a912).
  //   2. WorldViewSnapshot with the new queued task after the founder's
  //      `subtask_create` MCP call commits.
  //   3. WorldViewSnapshot with the task transitioned to in_progress and
  //      assignee_agent_id set to the engineer, after `scheduler::tick`.
  //
  // When the real-LLM e2e runner ships (M9.10) this becomes a live-traffic
  // proof; the rendering contract under test here doesn't change.
  test("§ 11.2 — operator directive arrives, subtask queues, engineer is assigned", async ({
    page,
  }) => {
    const STARTUP = "abcd1111-abcd-4111-abcd-111111111111";
    const FOUNDER = "f0000000-0000-4000-0000-000000000001";
    const ENGINEER = "e0000000-0000-4000-0000-000000000002";
    const DESIGNER = "d0000000-0000-4000-0000-000000000003";
    const TASK = "task-spec-001";

    await page.goto("/console");
    await stopWS(page);

    // Step 0: world publishes the startup + agents. No tasks yet — the
    // operator hasn't sent the directive.
    await dispatch(page, {
      type: "world_view_snapshot",
      snapshot: {
        startups: [
          { id: STARTUP, name: "alpha", budget_cap_usd: 100, budget_spent_usd: 0, last_event_ts: 1 },
        ],
        avatars: [
          { agent_id: FOUNDER, startup_id: STARTUP, role: "founder", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
          { agent_id: ENGINEER, startup_id: STARTUP, role: "engineer", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
          { agent_id: DESIGNER, startup_id: STARTUP, role: "designer", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
        ],
        tasks: [],
      },
    });

    // Select the startup so the kanban scopes to its tasks. Without this,
    // Kanban returns null (Kanban.tsx:38: `if (!startupId) return null;`).
    const sidebar = page.getByRole("complementary", { name: "startups" });
    await sidebar.locator(`[data-startup-id="${STARTUP}"]`).click();
    const main = page.locator("main");

    // Open the chat panel — directive frames land in state.messages and the
    // panel renders them.
    await page.getByRole("button", { name: "Open chat" }).click();
    const chat = page.getByRole("complementary", { name: "Chat" });
    await expect(chat).toBeVisible();

    // ── Step 1: operator sends a directive to the founder. The world's
    // `cmd_console::OperatorDirective` arm broadcasts a Directive frame
    // (M5; cmd_console.rs:67-141). Author is the sentinel "operator" so
    // `looksLikeUuid` (ChatPanel.tsx:202) renders it as raw text.
    await dispatch(page, {
      type: "directive",
      message_id: "msg-directive-001",
      ts: Date.now(),
      startup_id: STARTUP,
      author_id: "operator",
      to_agent_id: FOUNDER,
      body: "build a spec.md describing how to deploy",
      in_response_to_task: null,
    });

    // Bubble (ChatPanel.tsx:211-257) renders body + author + a directive arrow
    // tag (`tag = m.kind === "directive" ? "→" : "·"`). Assert all three so a
    // regression that flips kind→chat or drops the body trips this test.
    const directiveBubble = chat.getByText(
      "build a spec.md describing how to deploy",
    );
    await expect(directiveBubble).toBeVisible();
    // The "operator" sentinel renders as a raw <code> (no UUID truncation).
    await expect(chat.locator('code:text-is("operator")')).toHaveCount(1);
    // The directive-arrow tag (→) MUST be present; a chat-tag (·) MUST NOT
    // appear, since the only message in the panel is a directive. Catches
    // a regression where ChatPanel.tsx Bubble's `tag` switch flips kind.
    await expect(chat).toContainText("→");
    await expect(chat.locator(':text("·"):not(:has(*))')).toHaveCount(0);

    // ── Step 2: founder's CLI calls subtask_create; world transitions the
    // new task to `queued` and pushes a WorldViewSnapshot. The kanban's
    // "Queued" column (id="queued") gets a card.
    await dispatch(page, {
      type: "world_view_snapshot",
      snapshot: {
        startups: [
          { id: STARTUP, name: "alpha", budget_cap_usd: 100, budget_spent_usd: 0, last_event_ts: 2 },
        ],
        avatars: [
          { agent_id: FOUNDER, startup_id: STARTUP, role: "founder", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
          { agent_id: ENGINEER, startup_id: STARTUP, role: "engineer", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
          { agent_id: DESIGNER, startup_id: STARTUP, role: "designer", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
        ],
        tasks: [
          { id: TASK, startup_id: STARTUP, title: "Write spec.md", status: "queued", assignee_agent_id: null, required_room: null },
        ],
      },
    });

    const queuedColumn = main.locator('[data-column-id="queued"]');
    await expect(queuedColumn.getByText("Write spec.md")).toBeVisible();
    // Sanity: the in_progress column doesn't have it yet.
    await expect(
      main.locator('[data-column-id="in_progress"]').getByText("Write spec.md"),
    ).toHaveCount(0);

    // ── Step 3: scheduler::tick assigns the task to the engineer. Status
    // flips to `in_progress`, `assignee_agent_id` is set. The kanban moves
    // the card to the "In progress" column (id="in_progress") and the
    // assignee monogram derives from the engineer's agent_id (Card.tsx:50).
    await dispatch(page, {
      type: "world_view_snapshot",
      snapshot: {
        startups: [
          { id: STARTUP, name: "alpha", budget_cap_usd: 100, budget_spent_usd: 0, last_event_ts: 3 },
        ],
        avatars: [
          { agent_id: FOUNDER, startup_id: STARTUP, role: "founder", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
          { agent_id: ENGINEER, startup_id: STARTUP, role: "engineer", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "working" },
          { agent_id: DESIGNER, startup_id: STARTUP, role: "designer", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
        ],
        tasks: [
          { id: TASK, startup_id: STARTUP, title: "Write spec.md", status: "in_progress", assignee_agent_id: ENGINEER, required_room: null },
        ],
      },
    });

    const inProgressColumn = main.locator('[data-column-id="in_progress"]');
    await expect(inProgressColumn.getByText("Write spec.md")).toBeVisible();
    // Card no longer in queued column.
    await expect(
      queuedColumn.getByText("Write spec.md"),
    ).toHaveCount(0);
    // Card MUST display the engineer's monogram, not the "?" fallback.
    // Card.tsx:50 renders `monogramSrc = assignee?.agent_id ?? task.assignee_agent_id ?? "?"`,
    // so a regression where assignee isn't resolved (e.g. avatars lookup
    // fails or assignee_agent_id is dropped) renders "?". Asserting the
    // first char of ENGINEER ("e" → "E") and the absence of "?" pins the
    // assignee surface, not just the column.
    await expect(inProgressColumn.locator('text="E"')).toHaveCount(1);
    await expect(inProgressColumn.locator('text="?"')).toHaveCount(0);
  });

  // Card review-round badge: independent rendering check that pins the
  // contract Card.tsx::ReviewRoundBadge needs to satisfy for ship-gate
  // § 11.6's UI proof. Asserts that:
  //   - A task with review_round=0 (or undefined) renders NO badge.
  //   - A task with review_round=2, max=3 renders "R2/3" with a
  //     `data-review-round="2"` attribute the integration test can hook on.
  //   - At round=max (3/3), the title attribute reads "at escalation
  //     threshold" so the operator hover-explanation is verifiable.
  test("kanban card renders R{round}/{max} review-round badge", async ({
    page,
  }) => {
    const STARTUP = "ccdd1111-ccdd-4111-ccdd-111111111111";
    const TASK_FRESH = "task-fresh-001"; // review_round 0 → no badge
    const TASK_R2 = "task-r2-002"; // review_round 2 → "R2/3" amber
    const TASK_AT_CAP = "task-cap-003"; // review_round 3 → "R3/3" red, escalation tooltip

    await page.goto("/console");
    await stopWS(page);

    await dispatch(page, {
      type: "world_view_snapshot",
      snapshot: {
        startups: [
          { id: STARTUP, name: "alpha-rr", budget_cap_usd: 100, budget_spent_usd: 0, last_event_ts: 1 },
        ],
        avatars: [],
        tasks: [
          { id: TASK_FRESH, startup_id: STARTUP, title: "Fresh task no review", status: "queued", assignee_agent_id: null, required_room: null, review_round: 0, max_review_rounds: 3 },
          { id: TASK_R2, startup_id: STARTUP, title: "Task at round two", status: "awaiting_review", assignee_agent_id: null, required_room: null, review_round: 2, max_review_rounds: 3 },
          // status stays awaiting_review — escalation isn't yet surfaced in
          // the kanban (separate UI gap; tasks transitioning to "escalated"
          // disappear from the board today). The badge logic at round=cap
          // is what this test pins.
          { id: TASK_AT_CAP, startup_id: STARTUP, title: "Task at threshold", status: "awaiting_review", assignee_agent_id: null, required_room: null, review_round: 3, max_review_rounds: 3 },
        ],
      },
    });

    const sidebar = page.getByRole("complementary", { name: "startups" });
    await sidebar.locator(`[data-startup-id="${STARTUP}"]`).click();
    const main = page.locator("main");

    // Fresh task (round 0): no badge anywhere on its card. The card uses
    // data-review-round on the badge wrapper, so "no badge" === "no
    // attribute" within the queued column for that task title.
    const freshCard = main.getByText("Fresh task no review");
    await expect(freshCard).toBeVisible();
    // Sanity: no R0/N text rendered for fresh tasks (regression guard for
    // the `if (!round || round < 1) return null` early-exit).
    await expect(main.locator('text="R0/3"')).toHaveCount(0);

    // Round 2 task: badge with "R2/3" and data-review-round="2".
    const r2Badge = main.locator('[data-review-round="2"]');
    await expect(r2Badge).toHaveCount(1);
    await expect(r2Badge).toContainText("R2/3");

    // Round 3 (escalation threshold): "R3/3" + tooltip mentions threshold.
    const capBadge = main.locator('[data-review-round="3"]');
    await expect(capBadge).toHaveCount(1);
    await expect(capBadge).toContainText("R3/3");
    // Tooltip is the `title` attribute (also mirrored to aria-label).
    await expect(capBadge).toHaveAttribute("title", /escalation threshold/);
  });

  // § 11.4 — When the engineer's `task_done` MCP call commits, the world
  // updates the task row with `artifact_path = workspaces/<sid>/artifacts/
  // <tid>.md` AND status → awaiting_review in the same transaction. The
  // canonical path is then visible to the operator on the kanban card while
  // the manager reviews. Spec § 11.4.
  //
  // Filesystem proof (the file actually lands on disk at that path) lives in
  // the rust-layer `crates/world/tests/e2e_engineer_artifact.rs` — this UI
  // proof asserts the operator-observable surface: the canonical path string
  // renders on the card, attached to the right task.
  test("§ 11.4 — engineer artifact path renders on the kanban card", async ({
    page,
  }) => {
    const STARTUP = "1144aaaa-1144-4aaa-1144-aaaaaaaa1144";
    const ENGINEER = "1144eeee-eeee-4eee-eeee-eeeeeeeeeeee";
    const TASK_DONE_ID = "task-1144-art";
    const TASK_OPEN_ID = "task-1144-open";
    // Mirrors the world's canonical pattern from handle_task_done
    // (mcp_dispatch.rs::501-510). Any deviation is rejected with
    // bad_artifact_path, so the test pins both sides of the contract.
    const ARTIFACT = `workspaces/${STARTUP}/artifacts/${TASK_DONE_ID}.md`;

    await page.goto("/console");
    await stopWS(page);

    await dispatch(page, {
      type: "world_view_snapshot",
      snapshot: {
        startups: [
          { id: STARTUP, name: "artifact", budget_cap_usd: 100, budget_spent_usd: 0, last_event_ts: 1 },
        ],
        avatars: [
          { agent_id: ENGINEER, startup_id: STARTUP, role: "engineer", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
        ],
        tasks: [
          // Submitted task: engineer just called task_done, world flipped
          // status to awaiting_review with artifact_path attached. This is
          // the operator's window to see the path before the manager
          // accepts/rejects.
          { id: TASK_DONE_ID, startup_id: STARTUP, title: "Write spec.md", status: "awaiting_review", assignee_agent_id: ENGINEER, required_room: null, review_round: 0, max_review_rounds: 3, artifact_path: ARTIFACT },
          // Companion task with no artifact yet (still in_progress). Pins
          // the conditional render — `task.artifact_path && (...)` in
          // Card.tsx — so a regression that always renders the row trips
          // this test.
          { id: TASK_OPEN_ID, startup_id: STARTUP, title: "Open work item", status: "in_progress", assignee_agent_id: ENGINEER, required_room: null, review_round: 0, max_review_rounds: 3, artifact_path: null },
        ],
      },
    });

    const sidebar = page.getByRole("complementary", { name: "startups" });
    await sidebar.locator(`[data-startup-id="${STARTUP}"]`).click();
    const main = page.locator("main");

    // The submitted task lands in the awaiting_review column with the
    // canonical path attached. data-artifact-path lets the test pin the
    // exact card without grepping nearby DOM, and the visible text inside
    // <code> mirrors it for the operator's eyes.
    const reviewColumn = main.locator('[data-column-id="awaiting_review"]');
    const submittedCard = reviewColumn.getByText("Write spec.md");
    await expect(submittedCard).toBeVisible();
    const pathEl = main.locator(`[data-artifact-path="${ARTIFACT}"]`);
    await expect(pathEl).toHaveCount(1);
    await expect(pathEl).toContainText(ARTIFACT);
    // Canonical pattern guard: "workspaces/<startup_id>/artifacts/<task_id>.md".
    // Mirrors mcp_dispatch.rs:501-510 so a regression on either side trips.
    await expect(pathEl).toContainText(
      new RegExp(`^workspaces/[^/]+/artifacts/[^/]+\\.md$`),
    );
    // Tooltip mirrors the path so an operator hovering over a truncated
    // ellipsis can still read it. Pins both sides of `title` + visible text.
    await expect(pathEl).toHaveAttribute("title", ARTIFACT);

    // Companion (open work, no artifact yet): no artifact-path element
    // anywhere on its card. Asserting via the in_progress column scope
    // catches any regression that renders the path container always.
    const inProgressColumn = main.locator('[data-column-id="in_progress"]');
    await expect(inProgressColumn.getByText("Open work item")).toBeVisible();
    await expect(inProgressColumn.locator("[data-artifact-path]")).toHaveCount(0);
  });

  // § 11.6 — When a manager calls `task_request_changes`, the world commits
  // an UPDATE+INSERT transaction (round_++, directive INSERT) and broadcasts
  // a Directive ConsoleOutbound. After three rounds the next call hits the
  // escalation branch in `mcp_dispatch::handle_task_request_changes`: the
  // task transitions to status="escalated" and the world emits a
  // `task_escalated` SystemEvent with severity="alert". Spec § 11.6.
  //
  // The full chain (transactional integrity, branch selection, audit log)
  // lives in the rust-layer `crates/world/tests/console_emit.rs`. This UI
  // proof asserts the operator-observable transitions by synthesizing the
  // snapshot + system_event frames the world would emit:
  //   1. Round 0 in awaiting_review → no badge.
  //   2. After the first round-trip (engineer fixes after manager
  //      bounce-back), the same task reappears in awaiting_review with
  //      review_round=1 and the ReviewRoundBadge renders R1/3 yellow.
  //   3. At review_round=3, the badge tooltip warns "at escalation
  //      threshold".
  //   4. The (cap+1)-th `task_request_changes` triggers escalation: the
  //      world emits a `task_escalated` system_event (severity=alert,
  //      payload contains task_id + rounds) and the snapshot moves the
  //      task to status="escalated". The TopBar event-feed surfaces it
  //      and the History modal renders the full row.
  //
  // Phase-1 known gap: the kanban has no column for status="escalated"
  // (or "changes_requested"), so escalated tasks vanish from the board.
  // This test documents that gap by asserting the disappearance — when a
  // follow-up PR surfaces escalated tasks (dedicated column / critical
  // toast), update the assertion at the bottom of this test.
  test("§ 11.6 — review-cycle bumps badge round-by-round and escalates at cap", async ({
    page,
  }) => {
    const STARTUP = "eeff1111-eeff-4111-eeff-111111111111";
    const FOUNDER = "f1100000-0000-4000-0000-000000000001";
    const ENGINEER = "e1100000-0000-4000-0000-000000000002";
    const TASK = "task-review-cycle-001";

    await page.goto("/console");
    await stopWS(page);

    // Helper: build a snapshot for the same single-task fixture at a given
    // status + review_round. The status flip from awaiting_review →
    // changes_requested → awaiting_review during a round-trip is the
    // world's job; this test only renders the awaiting_review states the
    // operator actually sees the badge in.
    const snapshotAt = (status: string, reviewRound: number) => ({
      type: "world_view_snapshot",
      snapshot: {
        startups: [
          { id: STARTUP, name: "review-cycle", budget_cap_usd: 100, budget_spent_usd: 0, last_event_ts: reviewRound + 1 },
        ],
        avatars: [
          { agent_id: FOUNDER, startup_id: STARTUP, role: "founder", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
          { agent_id: ENGINEER, startup_id: STARTUP, role: "engineer", backend: "claude_code", current_pos: [0, 0], target_pos: null, room_id: "suite_1", status: "idle" },
        ],
        tasks: [
          { id: TASK, startup_id: STARTUP, title: "Review-cycle task", status, assignee_agent_id: ENGINEER, required_room: null, review_round: reviewRound, max_review_rounds: 3 },
        ],
      },
    });

    // Step 0: task is fresh in awaiting_review, no rounds yet.
    await dispatch(page, snapshotAt("awaiting_review", 0));

    const sidebar = page.getByRole("complementary", { name: "startups" });
    await sidebar.locator(`[data-startup-id="${STARTUP}"]`).click();
    const main = page.locator("main");
    const reviewColumn = main.locator('[data-column-id="awaiting_review"]');

    // Round 0: badge early-exits (`if (!round || round < 1) return null`),
    // so no `[data-review-round]` element exists anywhere on the card.
    await expect(reviewColumn.getByText("Review-cycle task")).toBeVisible();
    await expect(main.locator("[data-review-round]")).toHaveCount(0);

    // Step 1: manager request_changes (round 0 → 1) → engineer fixes →
    // task reappears in awaiting_review with review_round=1. Badge is now
    // visible in yellow at the lowest tier of escalation.
    await dispatch(page, snapshotAt("awaiting_review", 1));
    const r1 = main.locator('[data-review-round="1"]');
    await expect(r1).toHaveCount(1);
    await expect(r1).toContainText("R1/3");
    await expect(r1).toHaveAttribute("title", /Review round 1 of 3/);
    // Tooltip explicitly does NOT yet warn about the escalation threshold.
    // This pins the conditional suffix in `ReviewRoundBadge` (Card.tsx),
    // so a regression that always appends the warning trips the test.
    await expect(r1).not.toHaveAttribute("title", /escalation threshold/);

    // Step 2: jump to round 3 (cap). Round 2's color is pinned by the
    // standalone badge test above; this test cares about the cap signal.
    await dispatch(page, snapshotAt("awaiting_review", 3));
    const cap = main.locator('[data-review-round="3"]');
    await expect(cap).toHaveCount(1);
    await expect(cap).toContainText("R3/3");
    await expect(cap).toHaveAttribute("title", /at escalation threshold/);
    // Single card, single badge — the round-1 indicator is gone.
    await expect(main.locator('[data-review-round="1"]')).toHaveCount(0);

    // Step 3: cap+1 request hits the escalation branch in
    // `handle_task_request_changes`. The world emits a `task_escalated`
    // SystemEvent (severity=alert, kind=task_escalated) BEFORE the
    // snapshot transitions the task to status=escalated. Frontend
    // severity union (store.ts:57) accepts "alert" and "critical" after
    // M5.
    await dispatch(page, {
      type: "system_event",
      ts: Date.now(),
      severity: "alert",
      kind: "task_escalated",
      startup_id: STARTUP,
      payload: JSON.stringify({ task_id: TASK, rounds: 3, feedback: "still wrong" }),
    });
    await dispatch(page, snapshotAt("escalated", 3));

    // The TopBar event-feed (aria-label="event-feed") is the operator's
    // first-glance surface. The kind text reaches the rendered DOM,
    // proving the SystemEventVM made it through severityFromString.
    const feed = page.getByLabel("event-feed");
    await expect(feed).toContainText("task_escalated");

    // Open History modal, scope assertions to the dialog so they don't
    // accidentally match the topbar feed text.
    await page.getByRole("button", { name: "History" }).click();
    const dialog = page.getByRole("dialog", { name: "System event history" });
    await expect(dialog).toBeVisible();
    const eventRow = dialog.locator("li", { hasText: "task_escalated" });
    await expect(eventRow).toContainText("alert");
    await expect(eventRow).toContainText(TASK);
    await expect(eventRow).toContainText('"rounds":3');

    // Close the modal so it doesn't intercept the next assertion.
    await dialog.getByRole("button", { name: "Close" }).click();
    await expect(dialog).not.toBeVisible();

    // Phase-1 gap: status="escalated" has no kanban column, so the task
    // vanishes from the board. Document the gap; flip when surfaced.
    await expect(main.getByText("Review-cycle task")).toHaveCount(0);
  });
});
