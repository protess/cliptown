# P2.3 per-task execenv directories — design

**Date:** 2026-05-13
**Status:** draft — pending implementation
**Driver:** Phase 2 backlog second item from `docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md` § P2.3. cliptown's worker today passes a flat `--workspace` arg as adapter `cwd` — every task on the same agent shares the same filesystem context, with no place to inject per-task context files or skill content. This blocks P2.2 (skills) and makes "many tasks per agent" hostile to isolate.

## Goals

- Each `worker --real` spawn lands the adapter in a per-task workdir at `<workspaces_root>/workspaces/<sid>/<tid>/workdir/`.
- The workdir contains a `workspaces` symlink to `<workspaces_root>/workspaces` so the agent's existing relative artifact path (`workspaces/<sid>/artifacts/<tid>.md`) resolves to the canonical location without prompt changes.
- A minimal `CLAUDE.md` lands in the workdir at spawn time, telling the agent its identity, task id, and the canonical artifact path contract.
- Sibling dirs under `<sid>/<tid>/` are available for P2.2 skills content (out of scope here, but the layout reserves the space).
- Smoke + world + frontend tests stay green with no protocol changes.

## Non-goals (explicit)

- GC daemon for cleaning up old execenv dirs. Per spec, 7-day GC is "low priority"; deferred to a follow-up.
- Skills system (P2.2). The workdir layout reserves space for future `skills/` siblings, but no skill machinery ships here.
- Renaming `--workspace` to `--workspaces-root`. The flag's value is already the workspaces root semantically; renaming would churn smoke + external callers for no functional gain.
- Repo checkouts / per-task git clones (multica feature, not yet relevant to cliptown).
- Windows support for symlinks. macOS + Linux only; cliptown's real-LLM smoke doesn't run on Windows.

## Architecture

```
<workspaces_root>/workspaces/<sid>/
├── artifacts/<tid>.md                                  ← world-enforced canonical path
└── <tid>/                                              ← per-task execenv (NEW)
    ├── workdir/                                        ← adapter cwd
    │   ├── CLAUDE.md                                   ← injected context (NEW)
    │   └── workspaces -> <workspaces_root>/workspaces  ← absolute symlink (NEW)
    └── (P2.2 future: skills/, cache/, ...)
```

The symlink target is the absolute path of the workspaces tree
(`path.join(workspacesRoot, "workspaces")`) — explicit and safe against
any layout-depth change. Relative form (`../../..`) would work too but
adds a foot-gun if the workdir nesting ever changes; the directory move
cost on cliptown is zero (workspaces lives at the smoke tmpdir / a
stable absolute path), so absolute is the right pick here.

### Why the symlink

The agent's prompt — and the world's `task_done` validation — both reference `workspaces/<sid>/artifacts/<tid>.md` as a path relative to the agent's cwd. Today the agent's cwd is the workspaces root, so the path resolves directly to `<root>/workspaces/<sid>/artifacts/<tid>.md`. Moving cwd to the per-task workdir would break that resolution.

The symlink at `<workdir>/workspaces` pointing back to `<root>/workspaces` makes the agent's relative writes resolve correctly: `workspaces/<sid>/artifacts/<tid>.md` → through the symlink → `<root>/workspaces/<sid>/artifacts/<tid>.md`. No prompt change, no world change, no protocol change.

### Worker plumbing

`packages/worker/src/main.ts`:
- Adds `--task-id` flag (required, mirrors existing `--agent-id` / `--startup-id` pattern).
- Computes `workdir = path.join(workspacesRoot, "workspaces", startupId, taskId, "workdir")`.
- Calls a new `prepareWorkdir({ workspacesRoot, startupId, taskId, agentId })` helper.
- Passes `workdir` (not `workspacesRoot`) as `opts.cwd` to `adapter.spawn`.

`packages/worker/src/execenv.ts` *(new module)*:
- Pure-ish function with mkdir/symlink/writeFile side effects, no state.
- Steps inside `prepareWorkdir`:
  1. Ensure `<workspacesRoot>/workspaces` exists (defensive `mkdir -p`).
  2. Ensure `<workspacesRoot>/workspaces/<sid>/<tid>/workdir/` exists (`mkdir -p`).
  3. Create symlink `<workdir>/workspaces` → `../../../../workspaces` (idempotent: ignore EEXIST if it already points to the right target).
  4. Write `<workdir>/CLAUDE.md` with the templated context (always overwrite — content is deterministic from inputs).
  5. Return absolute path to `<workdir>`.

### CLAUDE.md content (Phase 2 MVP)

The injected file is small — task title/description live in the prompt; the file fixes the cwd-vs-canonical-path layout and the artifact contract.

```markdown
# Task context

You are agent `<agent_id>` running task `<task_id>` for startup `<startup_id>`.

## Working directory layout

- `./workspaces/` — symlink to the shared workspaces tree. The canonical artifact path for this task is `workspaces/<startup_id>/artifacts/<task_id>.md` (relative to this workdir).
- Anything else you create directly in this workdir is per-task scratch and survives the session until GC.

## When you're done

Call the MCP tool `task_done` with `task_id = "<task_id>"` and `artifact_path = "workspaces/<startup_id>/artifacts/<task_id>.md"`. The world enforces this exact path.
```

No links to other files, no nested heading hierarchy. Phase 2 keeps this lean; P2.2 (skills) will add structured skill-attached sections.

## Boundaries / dependencies

- **No world-side changes.** `mcp_dispatch::handle_task_done` keeps its canonical-path string check + `sandbox::resolve` belt-and-suspenders.
- **No adapter changes.** All three adapters (claude-code, codex, opencode) already pass `opts.cwd` through to the spawned child. The new workdir is just a different cwd value.
- **No protocol changes.** `BackendAdapter.SpawnOpts` already carries `cwd: string`.

## Worker arg shape

```ts
interface ParsedArgs {
  worldUrl: string;
  agentId: string;
  startupId: string;
  taskId: string;            // NEW — required
  secret: string;
  backend: string;
  workspace: string;
  prompt: string;
  mock?: boolean;
  real?: boolean;
  fixture?: string;
}
```

`mock` mode still requires `--task-id` (mock can use any value, e.g. `mock-task`). Worker entry validates presence; cleaner than making it conditional on `--real`.

## Tests

### Unit (`packages/worker/test/execenv.test.ts`)

Four tests using `tempfile` / `node:fs/promises`:

1. **`prepareWorkdir creates workdir hierarchy`** — assert `<root>/workspaces/<sid>/<tid>/workdir/` exists as a directory after the call.
2. **`prepareWorkdir creates workspaces symlink targeting root`** — assert `fs.lstat(workdir/workspaces).isSymbolicLink()` and `fs.realpath(workdir/workspaces) === path.resolve(root, "workspaces")`.
3. **`prepareWorkdir writes CLAUDE.md with task context`** — assert file exists; content contains `agent_id`, `task_id`, `startup_id`, and the canonical artifact path string `workspaces/<sid>/artifacts/<tid>.md`.
4. **`prepareWorkdir is idempotent`** — call twice with the same inputs; second call must not throw and must leave the dir / symlink / CLAUDE.md unchanged.

### Worker args (`packages/worker/test/main_args.test.ts`)

One added case: `--task-id` is required; absence triggers the same "missing required" path as other required flags.

### Smoke (`scripts/smoke-real-llm.sh`)

- Add `--task-id "$TASK_ID"` to the worker spawn line.
- Add a verification line after the existing artifact check: confirm `workspaces/<sid>/$TASK_ID/workdir/CLAUDE.md` exists and contains the canonical artifact path string; confirm `workspaces/<sid>/$TASK_ID/workdir/workspaces` is a symlink.

### Contract test (`packages/worker/test/contract.test.ts`)

Untouched. Contract tests call `adapter.spawn` directly with `cwd: tmpdir()` and don't go through worker entry, so `prepareWorkdir` doesn't run. No regression.

## Migration / risk

- `--task-id` is a new required arg. The only current caller (`scripts/smoke-real-llm.sh`) needs the one-line update.
- Symlink dangling: `prepareWorkdir` runs `mkdir -p` for `<root>/workspaces` before creating the symlink, so the target always exists. World's `api_startups::create_startup` already mkdirs `workspaces/<sid>/artifacts/` ahead of any worker spawn, so the actual artifact dir is also there.
- No GC: each smoke run leaves the per-task dir on disk. For the local smoke harness this is fine (smoke runs in tmpdir, cleaned up at script exit unless `KEEP_TMP=1`). Known limitation noted in CHANGELOG; future GC daemon handles long-running deployments.
- Symlink + sandboxed CLI: claude-code / codex / opencode all open files through their own filesystem APIs. None of them have observed issues following symlinks in cwd. Real-LLM smoke verifies.

## Definition of done

- `packages/worker/src/execenv.ts` exists with `prepareWorkdir` helper.
- `packages/worker/src/main.ts` parses `--task-id`, computes workdir via `prepareWorkdir`, spawns adapter with `cwd: workdir`.
- `packages/worker/test/execenv.test.ts` — 4 tests green.
- `packages/worker/test/main_args.test.ts` — 1 added case green; full suite stays at 65 + 1 + 4 = 70.
- `scripts/smoke-real-llm.sh` — adds `--task-id`, adds the workdir/symlink/CLAUDE.md verification line.
- `BACKEND=claude_code BUDGET_CAP_USD=1.0 bash scripts/smoke-real-llm.sh` — passes; artifact lands at canonical path; new verification line passes.
- `cargo test -p cliptown-world` — 231 unchanged (no world code touched).
- `pnpm -F @cliptown/adapter-{core,claude-code,codex,opencode} test` — 35 unchanged.
- `pnpm -F @cliptown/frontend e2e` — 14 unchanged.
- `node bench/check.mjs` — gate green (no Rust changes).
- CHANGELOG M12 P2.3 section + TODOS Completed entry.
