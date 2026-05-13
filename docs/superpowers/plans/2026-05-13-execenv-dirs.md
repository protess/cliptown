# P2.3 per-task execenv directories implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Worker creates a per-task execenv directory at `<workspaces_root>/workspaces/<sid>/<tid>/workdir/` (with a `workspaces` symlink pointing back to the canonical workspaces tree, and an injected CLAUDE.md carrying task context) and spawns the adapter with that workdir as cwd. No protocol or world changes; the agent's existing relative artifact path resolves through the symlink to the canonical location.

**Architecture:** New `packages/worker/src/execenv.ts` module owns the mkdir + symlink + writeFile side effects in one idempotent `prepareWorkdir` helper. `packages/worker/src/main.ts` parses a new `--task-id` arg, calls `prepareWorkdir` in the `--real` branch right before adapter spawn, and passes the returned workdir as `opts.cwd`. Smoke script picks up the new flag and adds a workdir/symlink/CLAUDE.md verification line.

**Tech Stack:** TypeScript, Node 18+ (`node:fs/promises`, `node:path`, `node:util` parseArgs), vitest.

**Spec:** `docs/superpowers/specs/2026-05-13-execenv-dirs-design.md`

---

## File structure

- `packages/worker/src/execenv.ts` *(new)* — exports `prepareWorkdir({ workspacesRoot, startupId, taskId, agentId }) → Promise<string>`. Side effects only: mkdir, symlink, writeFile. Idempotent: re-runs on the same inputs are safe.
- `packages/worker/test/execenv.test.ts` *(new)* — 4 tests using `tempfile` + `node:fs/promises` to verify hierarchy / symlink target / CLAUDE.md content / idempotency.
- `packages/worker/src/main.ts` *(modify)* — `ParsedArgs` gains `taskId: string`; `parseWorkerArgs` requires `--task-id`; the `--real` branch calls `prepareWorkdir` and uses the returned workdir as `adapter.spawn`'s `cwd`.
- `packages/worker/test/main_args.test.ts` *(modify)* — every `baseArgs` builder picks up `--task-id` so existing tests still parse; one new "throws on missing --task-id" case added.
- `scripts/smoke-real-llm.sh` *(modify)* — adds `--task-id "$TASK_ID"` to the worker spawn line; adds three verification assertions (workdir exists, `workspaces` symlink exists and resolves to canonical, CLAUDE.md contains canonical artifact path string).
- `CHANGELOG.md` + `TODOS.md` *(modify)* — M12 P2.3 section atop CHANGELOG; TODOS Completed entry with `<TBD>` PR placeholder.

---

## Task 1: `execenv.ts` failing tests (TDD red)

**Files:**
- Create: `packages/worker/test/execenv.test.ts`

- [ ] **Step 1: Create the test file**

Use the Write tool to create `packages/worker/test/execenv.test.ts` with EXACTLY this content:

```ts
import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm, lstat, realpath, readFile, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { prepareWorkdir } from "../src/execenv.js";

describe("prepareWorkdir", () => {
  let root: string;

  beforeEach(async () => {
    root = await mkdtemp(join(tmpdir(), "ct-execenv-"));
  });

  afterEach(async () => {
    await rm(root, { recursive: true, force: true });
  });

  it("creates the workdir hierarchy at <root>/workspaces/<sid>/<tid>/workdir/", async () => {
    const workdir = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
    });
    const expected = resolve(root, "workspaces", "s1", "t1", "workdir");
    expect(workdir).toBe(expected);
    const st = await stat(workdir);
    expect(st.isDirectory()).toBe(true);
  });

  it("creates the workspaces symlink resolving to <root>/workspaces", async () => {
    const workdir = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
    });
    const linkPath = join(workdir, "workspaces");
    const lst = await lstat(linkPath);
    expect(lst.isSymbolicLink()).toBe(true);
    const target = await realpath(linkPath);
    expect(target).toBe(resolve(root, "workspaces"));
  });

  it("writes CLAUDE.md with agent/task/startup context + canonical artifact path", async () => {
    const workdir = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
    });
    const content = await readFile(join(workdir, "CLAUDE.md"), "utf-8");
    expect(content).toContain("a1");
    expect(content).toContain("t1");
    expect(content).toContain("s1");
    expect(content).toContain("workspaces/s1/artifacts/t1.md");
  });

  it("is idempotent — second call with same inputs does not throw", async () => {
    const first = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
    });
    const second = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
    });
    expect(second).toBe(first);
    const lst = await lstat(join(second, "workspaces"));
    expect(lst.isSymbolicLink()).toBe(true);
  });
});
```

- [ ] **Step 2: Verify the test fails for the right reason**

Run from the repo root:

```bash
pnpm -F @cliptown/worker test -- --run execenv.test.ts 2>&1 | tail -10
```

Expected: FAIL — module `../src/execenv.js` not found. This is the red phase of TDD.

- [ ] **Step 3: Do NOT commit yet**

Task 2 commits both the test file and the implementation together.

---

## Task 2: `execenv.ts` implementation (TDD green) + commit

**Files:**
- Create: `packages/worker/src/execenv.ts`

- [ ] **Step 1: Write the module**

Use the Write tool to create `packages/worker/src/execenv.ts` with EXACTLY this content:

```ts
import { mkdir, symlink, writeFile, readlink } from "node:fs/promises";
import { join, resolve } from "node:path";

/**
 * P2.3 per-task execenv: prepare an isolated workdir for the adapter.
 *
 * Layout at <workspacesRoot>/workspaces/<startupId>/<taskId>/:
 *   workdir/
 *     CLAUDE.md           — injected task context
 *     workspaces -> <workspacesRoot>/workspaces (absolute symlink)
 *
 * Returns the absolute path to the workdir. Idempotent — calling twice
 * with the same inputs is a no-op (existing directory / symlink / file
 * are left in place if they already match the expected shape).
 */

export interface PrepareWorkdirOpts {
  /** Absolute path passed via --workspace; the parent of the workspaces tree. */
  workspacesRoot: string;
  startupId: string;
  taskId: string;
  agentId: string;
}

export async function prepareWorkdir(opts: PrepareWorkdirOpts): Promise<string> {
  const wsRoot = resolve(opts.workspacesRoot);
  const workspacesDir = join(wsRoot, "workspaces");
  const workdir = join(workspacesDir, opts.startupId, opts.taskId, "workdir");

  // Ensure the entire chain up to workdir/ exists. mkdir recursive doesn't
  // throw on existing dirs.
  await mkdir(workdir, { recursive: true });
  // Also ensure the symlink target exists (defensive — world's
  // api_startups normally creates this before the worker spawns).
  await mkdir(workspacesDir, { recursive: true });

  // Symlink at <workdir>/workspaces → <wsRoot>/workspaces (absolute).
  // If it already exists with the correct target, leave it alone.
  const linkPath = join(workdir, "workspaces");
  try {
    await symlink(workspacesDir, linkPath);
  } catch (e) {
    const err = e as NodeJS.ErrnoException;
    if (err.code !== "EEXIST") throw err;
    // Verify the existing entry points where we expect; if not, replace.
    let existing: string | null = null;
    try {
      existing = await readlink(linkPath);
    } catch {
      existing = null;
    }
    if (existing !== workspacesDir) {
      // Stale link or non-link file at the path. Don't overwrite —
      // surface the conflict so the operator notices.
      throw new Error(
        `workdir/workspaces exists but doesn't point to ${workspacesDir} (got ${existing ?? "non-link entry"})`,
      );
    }
  }

  // CLAUDE.md is always rewritten — content is deterministic from inputs,
  // so this is safe to call repeatedly.
  const claudeMd = buildClaudeMd(opts);
  await writeFile(join(workdir, "CLAUDE.md"), claudeMd, "utf-8");

  return workdir;
}

function buildClaudeMd(opts: PrepareWorkdirOpts): string {
  const { agentId, taskId, startupId } = opts;
  const canonical = `workspaces/${startupId}/artifacts/${taskId}.md`;
  return [
    "# Task context",
    "",
    `You are agent \`${agentId}\` running task \`${taskId}\` for startup \`${startupId}\`.`,
    "",
    "## Working directory layout",
    "",
    "- `./workspaces/` — symlink to the shared workspaces tree. The canonical artifact path for this task is `" +
      canonical +
      "` (relative to this workdir).",
    "- Anything else you create directly in this workdir is per-task scratch and survives the session until GC.",
    "",
    "## When you're done",
    "",
    "Call the MCP tool `task_done` with `task_id = \"" +
      taskId +
      "\"` and `artifact_path = \"" +
      canonical +
      "\"`. The world enforces this exact path.",
    "",
  ].join("\n");
}
```

- [ ] **Step 2: Run the new tests + the rest of the worker suite**

```bash
pnpm -F @cliptown/worker test 2>&1 | tail -10
```

Expected:
- 4 new `execenv.test.ts` tests pass.
- All existing tests pass (was 65, now 65 + 4 = 69; the `--task-id` requirement lands in Task 3, so `main_args.test.ts` is still on the old shape here).

- [ ] **Step 3: Commit**

```bash
git add packages/worker/src/execenv.ts packages/worker/test/execenv.test.ts
git commit -m "$(cat <<'EOF'
feat(worker): prepareWorkdir for per-task execenv directories

Pure-ish helper creates <wsRoot>/workspaces/<sid>/<tid>/workdir/ with
an absolute symlink workdir/workspaces -> <wsRoot>/workspaces and an
injected CLAUDE.md carrying agent_id / task_id / startup_id + the
canonical artifact path contract. Idempotent: re-running on the same
inputs is a no-op. Returns the absolute workdir path for the caller
to pass as adapter cwd.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Worker args — `--task-id` becomes required

**Files:**
- Modify: `packages/worker/src/main.ts:14-72`
- Modify: `packages/worker/test/main_args.test.ts`

- [ ] **Step 1: Add `--task-id` to the failing test**

Open `packages/worker/test/main_args.test.ts`. The first describe block has a `baseArgs` array around line 5. Add `--task-id` to it:

```ts
  const baseArgs = [
    "--world-url", "ws://localhost:8080/ws/worker",
    "--agent-id", "a1",
    "--startup-id", "s1",
    "--task-id", "t1",
    "--secret", "shh",
    "--workspace", "/tmp/ws/s1",
  ];
```

Update the "parses all required args" test to assert `taskId`:

```ts
  it("parses all required args", () => {
    const a = parseWorkerArgs(baseArgs);
    expect(a.worldUrl).toBe("ws://localhost:8080/ws/worker");
    expect(a.agentId).toBe("a1");
    expect(a.startupId).toBe("s1");
    expect(a.taskId).toBe("t1");
    expect(a.secret).toBe("shh");
    expect(a.workspace).toBe("/tmp/ws/s1");
    expect(a.backend).toBe("claude_code");
    expect(a.mock).toBe(false);
    expect(a.fixture).toBeUndefined();
    expect(a.prompt).toBe("");
    expect(a.real).toBe(false);
  });
```

Add a new "throws on missing --task-id" test, placed alongside the other "throws on missing" tests (e.g., after the `--agent-id` missing test):

```ts
  it("throws on missing --task-id", () => {
    const args = baseArgs.filter((_, i, arr) => !(arr[i-1] === "--task-id" || arr[i] === "--task-id"));
    expect(() => parseWorkerArgs(args)).toThrow(/missing required arg --task-id/);
  });
```

- [ ] **Step 2: Run the test to confirm it fails**

```bash
pnpm -F @cliptown/worker test -- --run main_args.test.ts 2>&1 | tail -15
```

Expected: FAIL — `parseWorkerArgs` doesn't recognize `--task-id`, and the new test for missing `--task-id` fails because there's no validation yet.

- [ ] **Step 3: Update `parseWorkerArgs` in main.ts**

Open `packages/worker/src/main.ts`. Find the `ParsedArgs` interface (around line 14) and add `taskId`:

```ts
export interface ParsedArgs {
  worldUrl: string;
  agentId: string;
  startupId: string;
  taskId: string;
  secret: string;
  backend: string;
  workspace: string;
  mock: boolean;
  fixture: string | undefined;
  prompt: string;
  real: boolean;
}
```

Then in `parseWorkerArgs` (around line 32), add `"task-id"` to the options block:

```ts
    options: {
      "world-url":  { type: "string" },
      "agent-id":   { type: "string" },
      "startup-id": { type: "string" },
      "task-id":    { type: "string" },
      "secret":     { type: "string" },
      "backend":    { type: "string", default: "claude_code" },
      "workspace":  { type: "string" },
      "mock":       { type: "boolean", default: false },
      "fixture":    { type: "string" },
      "prompt":     { type: "string", default: "" },
      "real":       { type: "boolean", default: false },
    },
```

Update the returned object to require it:

```ts
  return {
    worldUrl:  required("world-url",  values["world-url"]),
    agentId:   required("agent-id",   values["agent-id"]),
    startupId: required("startup-id", values["startup-id"]),
    taskId:    required("task-id",    values["task-id"]),
    secret:    required("secret",     values["secret"]),
    backend:   String(values["backend"]),
    workspace: required("workspace",  values["workspace"]),
    mock:      Boolean(values["mock"]),
    fixture:   typeof values["fixture"] === "string" ? values["fixture"] : undefined,
    prompt:    String(values["prompt"]),
    real:      Boolean(values["real"]),
  };
```

- [ ] **Step 4: Run the test to confirm green**

```bash
pnpm -F @cliptown/worker test 2>&1 | tail -5
```

Expected: all worker tests pass. Count: 65 (original) + 4 (execenv) + 1 (new task-id missing test) = 70.

- [ ] **Step 5: Commit**

```bash
git add packages/worker/src/main.ts packages/worker/test/main_args.test.ts
git commit -m "$(cat <<'EOF'
feat(worker): --task-id is required

ParsedArgs gains taskId: string and parseWorkerArgs requires --task-id.
Wires into the upcoming prepareWorkdir call in the --real spawn path.
All existing main_args tests get the new arg added to baseArgs; one
new test asserts the missing-arg fast-fail path.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Wire `prepareWorkdir` into worker `--real` spawn path

**Files:**
- Modify: `packages/worker/src/main.ts:199-235` (the `--real` branch)

- [ ] **Step 1: Add the prepareWorkdir import**

At the top of `packages/worker/src/main.ts`, alongside the existing imports, add:

```ts
import { prepareWorkdir } from "./execenv.js";
```

- [ ] **Step 2: Update the `--real` branch to call prepareWorkdir**

Find the `} else if (args.real) {` block (around line 199 of main.ts). The current shape is:

```ts
  } else if (args.real) {
    // M9.10 A2 — one-shot real-LLM mode. ...
    const adapter = pickAdapter(args.backend);
    // ... compute mcpWorldUrl + mcpToken ...
    console.log(
      `[worker] real mode: spawning ${args.backend} → MCP @ ${mcpWorldUrl}/mcp`,
    );
    const spawned = await adapter.spawn({
      prompt: args.prompt,
      cwd: workspaceRoot,
      mcp_world_url: mcpWorldUrl,
      mcp_token: mcpToken,
      // ... onHook, onLog ...
    });
```

Replace the `cwd: workspaceRoot,` line with the execenv path. Before the `adapter.spawn` call, add a `prepareWorkdir` call. The final shape should be:

```ts
  } else if (args.real) {
    // M9.10 A2 — one-shot real-LLM mode. Worker becomes a process supervisor:
    // spawn the adapter, log hooks + stdio, wait for CLI exit, close WS, done.
    // MCP traffic flows CLI → world `/mcp` (HTTP) directly per A1' — the
    // worker's `McpProxy` is unused in this path.
    const adapter = pickAdapter(args.backend);
    // ... existing httpBase / mcpWorldUrl / mcpToken setup unchanged ...

    // P2.3: per-task execenv. Creates <wsRoot>/workspaces/<sid>/<tid>/workdir/
    // with an absolute symlink workdir/workspaces → <wsRoot>/workspaces and an
    // injected CLAUDE.md. The agent's existing relative artifact path
    // (workspaces/<sid>/artifacts/<tid>.md) resolves through the symlink to
    // the canonical location without prompt or world changes.
    const workdir = await prepareWorkdir({
      workspacesRoot: workspaceRoot,
      startupId: args.startupId,
      taskId: args.taskId,
      agentId: args.agentId,
    });
    console.log(
      `[worker] real mode: spawning ${args.backend} → MCP @ ${mcpWorldUrl}/mcp (cwd=${workdir})`,
    );
    const spawned = await adapter.spawn({
      prompt: args.prompt,
      cwd: workdir,
      mcp_world_url: mcpWorldUrl,
      mcp_token: mcpToken,
      onHook: (e) => console.log(`[worker] hook: ${e.kind} tool=${e.tool}`),
      onLog: (stream, line) => {
        const out = stream === "stderr" ? process.stderr : process.stdout;
        out.write(`[${args.backend}] ${line}`);
      },
    });
```

(Preserve every other line in the `--real` branch — `httpBase`, `mcpWorldUrl`, `mcpToken`, the `await spawned.wait()` and post-exit budget telemetry logic stay exactly as they are.)

- [ ] **Step 3: Run worker tests**

```bash
pnpm -F @cliptown/worker test 2>&1 | tail -5
```

Expected: 70 tests still pass. No new tests in this step — the main.ts wiring is exercised end-to-end by the smoke script in Task 5.

- [ ] **Step 4: Commit**

```bash
git add packages/worker/src/main.ts
git commit -m "$(cat <<'EOF'
feat(worker): real mode spawns adapter in per-task workdir

The --real branch now calls prepareWorkdir before adapter.spawn and
passes the returned workdir (instead of the workspaces root) as cwd.
Mock mode is unchanged. The workdir's absolute symlink workdir/
workspaces -> <wsRoot>/workspaces means the agent's existing relative
artifact path (workspaces/<sid>/artifacts/<tid>.md) resolves to the
canonical location without prompt or world changes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Smoke script — `--task-id` + workdir verification

**Files:**
- Modify: `scripts/smoke-real-llm.sh`

- [ ] **Step 1: Add `--task-id` to the worker spawn**

Open `scripts/smoke-real-llm.sh`. Find the worker spawn block (around line 204). Add `--task-id "$TASK_ID"` to the args list (TASK_ID is already declared at the top of the script). The block becomes:

```bash
  pnpm -F @cliptown/worker exec tsx ./src/main.ts \
    --world-url "ws://$WORLD_BIND/ws/worker" \
    --agent-id "$ENGINEER_ID" \
    --startup-id "$STARTUP_ID" \
    --task-id "$TASK_ID" \
    --secret "$AGENT_SECRET" \
    --backend "$BACKEND" \
    --workspace "$SMOKE_DIR" \
    --real \
    --prompt "$PROMPT" \
    >"$WORKER_LOG" 2>&1
```

- [ ] **Step 2: Add the workdir / symlink / CLAUDE.md verification block**

Find the existing artifact verification block (search for `verify: artifact on disk at`). Right after that block ends (before the budget-cap check, which begins with `# ── 7.5` or `say "verify: budget"`), insert a new verification:

```bash
# ── 7.5. verify: per-task execenv (P2.3) ───────────────────────────────────
say "verify: per-task execenv at workspaces/$STARTUP_ID/$TASK_ID/workdir/"
EXECENV_WORKDIR="$SMOKE_DIR/workspaces/$STARTUP_ID/$TASK_ID/workdir"
[[ -d "$EXECENV_WORKDIR" ]] || fail "workdir not found: $EXECENV_WORKDIR"
[[ -L "$EXECENV_WORKDIR/workspaces" ]] || fail "workspaces symlink missing inside workdir"
LINK_TARGET="$(readlink "$EXECENV_WORKDIR/workspaces")"
EXPECTED_TARGET="$SMOKE_DIR/workspaces"
[[ "$LINK_TARGET" == "$EXPECTED_TARGET" ]] || fail "symlink target mismatch: got $LINK_TARGET, expected $EXPECTED_TARGET"
CLAUDE_MD="$EXECENV_WORKDIR/CLAUDE.md"
[[ -f "$CLAUDE_MD" ]] || fail "CLAUDE.md missing at $CLAUDE_MD"
grep -q "workspaces/$STARTUP_ID/artifacts/$TASK_ID.md" "$CLAUDE_MD" \
  || fail "CLAUDE.md does not reference canonical artifact path"
say "execenv check passed: workdir + symlink + CLAUDE.md all present"
```

Use whatever section numbering matches the existing pattern in the script. If the existing sections are numbered `# ── 7.`, `# ── 7.5.` slots in cleanly before the budget check.

- [ ] **Step 3: Run the smoke locally to confirm**

This step costs ~$0.30 in real claude API usage; only run if the cost is acceptable. If the user wants to skip the local smoke during dev and let CI catch it, skip Step 3 and proceed to Step 4.

```bash
KEEP_TMP=1 BUDGET_CAP_USD=1.0 BACKEND=claude_code bash scripts/smoke-real-llm.sh 2>&1 | tail -20
```

Expected: smoke ends with `PASS — A3 smoke complete`. The new "execenv check passed" line appears.

- [ ] **Step 4: Commit**

```bash
git add scripts/smoke-real-llm.sh
git commit -m "$(cat <<'EOF'
chore(smoke): pass --task-id; verify per-task execenv on disk

Adds --task-id "$TASK_ID" to the worker spawn line (now required after
P2.3). Adds a verification block that asserts the per-task workdir
exists at workspaces/$STARTUP_ID/$TASK_ID/workdir/, the workspaces
symlink inside points to $SMOKE_DIR/workspaces, and CLAUDE.md
references the canonical artifact path.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: CHANGELOG + TODOS + verification sweep

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `TODOS.md`

- [ ] **Step 1: Insert the M12 P2.3 section atop CHANGELOG.md**

Find the existing `## M12 — P2.1 daemon health buckets (2026-05-12)` heading. Insert this NEW section ABOVE it (so the latest entry sits on top):

```markdown
## M12 — P2.3 per-task execenv directories (2026-05-13)

Worker now creates a per-task execenv directory at
`<workspaces_root>/workspaces/<sid>/<tid>/workdir/` before each
`--real` adapter spawn, and uses that workdir as the adapter's cwd.

- **`packages/worker/src/execenv.ts`** (new) — `prepareWorkdir({
  workspacesRoot, startupId, taskId, agentId })` creates the workdir
  hierarchy, an absolute symlink `workdir/workspaces` →
  `<workspaces_root>/workspaces`, and an injected `CLAUDE.md` with
  agent_id / task_id / startup_id + the canonical artifact path
  contract. Idempotent.
- **`--task-id`** is now a required worker arg. The smoke script
  passes the existing `TASK_ID` value through.
- **No protocol / world / adapter changes.** The symlink lets the
  agent's existing relative path `workspaces/<sid>/artifacts/<tid>.md`
  resolve to the canonical location without touching the prompt or
  the world's `task_done` validator.
- **Reserves space for P2.2 (skills).** Sibling dirs under
  `<sid>/<tid>/` (e.g., `skills/`, `cache/`) are layout-ready; this
  PR doesn't ship any skill machinery.

### Known limitations carried forward

- No GC daemon for old execenv directories. Local smoke cleans up via
  tmpdir; long-running deployments will need a sweeper (separate
  follow-up).

```

- [ ] **Step 2: Add the TODOS Completed entry**

Open `TODOS.md`. Under `## Completed`, ABOVE the existing `### M12 P2.1 daemon health buckets` entry, insert:

```markdown
### M12 P2.3 per-task execenv directories — 2026-05-13
**Source:** Phase 2 backlog second item (from `docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md` § P2.3). PR `<TBD — fill in at PR creation>`.

Was: worker passed a flat `--workspace` arg to every adapter spawn — every task on the same agent shared the same filesystem context, with no place to inject per-task context files or skill content. This blocked P2.2 (skills) and made "many tasks per agent" hostile to isolate.

Fixed: new `packages/worker/src/execenv.ts::prepareWorkdir` creates `<workspaces_root>/workspaces/<sid>/<tid>/workdir/` per task, with an absolute symlink `workdir/workspaces` → `<workspaces_root>/workspaces` so the agent's existing relative artifact path resolves through the symlink to the canonical location (no prompt or world change). A minimal `CLAUDE.md` lands in the workdir at spawn carrying agent_id / task_id / startup_id + the canonical artifact path contract. Worker's `--task-id` is now required. GC daemon deferred — known limitation in CHANGELOG.

```

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md TODOS.md
git commit -m "$(cat <<'EOF'
docs: M12 P2.3 per-task execenv directories changelog + TODOS

Adds the M12 P2.3 section atop CHANGELOG. TODOS Completed gets the
matching entry with TBD PR placeholder (filled at PR creation).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 4: Full verification sweep**

Run from the repo root:

```bash
cargo test -p cliptown-world 2>&1 | grep "test result:" | awk '{sum += $4} END {print "rust:", sum}'
pnpm -F @cliptown/adapter-core test 2>&1 | grep -E "Tests *[0-9]+ passed" | head -1
pnpm -F @cliptown/adapter-claude-code test 2>&1 | grep -E "Tests *[0-9]+ passed" | head -1
pnpm -F @cliptown/adapter-codex test 2>&1 | grep -E "Tests *[0-9]+ passed" | head -1
pnpm -F @cliptown/adapter-opencode test 2>&1 | grep -E "Tests *[0-9]+ passed" | head -1
pnpm -F @cliptown/worker test 2>&1 | grep -E "Tests *[0-9]+ passed" | head -1
pnpm -F @cliptown/frontend e2e 2>&1 | tail -2 | head -1
node bench/check.mjs 2>&1 | python3 -c "import json,sys; d=json.load(sys.stdin); print(f'check.mjs ok={d[\"ok\"]}')"
```

Expected:
- rust: 231 (unchanged — no world code touched)
- adapter-core: 3
- adapter-claude-code: 8
- adapter-codex: 12
- adapter-opencode: 12
- worker: 70 (was 65, +4 execenv +1 task-id missing)
- frontend e2e: 14 passed
- bench/check.mjs: ok=True (no Rust changes)

No commit in Step 4 — pure verification.

---

## Definition of done

- `packages/worker/src/execenv.ts` exists with `prepareWorkdir` exported; 4 inline unit tests pass.
- Worker `--task-id` is required; parseWorkerArgs validates; main_args.test.ts adds the missing-arg case.
- Worker `--real` branch calls `prepareWorkdir` and passes the returned workdir to `adapter.spawn` as `cwd`.
- Smoke script passes `--task-id` and verifies workdir + symlink + CLAUDE.md presence + canonical path mention.
- `cargo test -p cliptown-world` — 231 unchanged.
- `pnpm -F @cliptown/worker test` — 70 (was 65, +5).
- `pnpm -F @cliptown/adapter-{core,claude-code,codex,opencode} test` — 35 unchanged.
- `pnpm -F @cliptown/frontend e2e` — 14 unchanged.
- `node bench/check.mjs` — ok=true.
- CHANGELOG carries the M12 P2.3 section; TODOS Completed has the matching entry (PR # filled at PR-create time).
