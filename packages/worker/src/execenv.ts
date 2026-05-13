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
