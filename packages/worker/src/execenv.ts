import { mkdir, symlink, writeFile, readlink } from "node:fs/promises";
import { join, resolve } from "node:path";

/**
 * P2.3 per-task execenv: prepare an isolated workdir for the adapter.
 *
 * Layout at <workspacesRoot>/workspaces/<startupId>/<taskId>/:
 *   workdir/
 *     CLAUDE.md           — injected task context
 *     workspaces -> <workspacesRoot>/workspaces (absolute symlink)
 *     skills/             — optional; one <name>.md per attached skill
 *
 * Returns the absolute path to the workdir. Idempotent — calling twice
 * with the same inputs is a no-op (existing directory / symlink / file
 * are left in place if they already match the expected shape).
 */

export interface SkillContent {
  name: string;
  content_md: string;
}

export interface PrepareWorkdirOpts {
  /** Absolute path passed via --workspace; the parent of the workspaces tree. */
  workspacesRoot: string;
  startupId: string;
  taskId: string;
  agentId: string;
  skills?: SkillContent[];
}

export async function prepareWorkdir(opts: PrepareWorkdirOpts): Promise<string> {
  const wsRoot = resolve(opts.workspacesRoot);
  const workspacesDir = join(wsRoot, "workspaces");
  const workdir = join(workspacesDir, opts.startupId, opts.taskId, "workdir");

  await mkdir(workdir, { recursive: true });
  await mkdir(workspacesDir, { recursive: true });

  const linkPath = join(workdir, "workspaces");
  try {
    await symlink(workspacesDir, linkPath);
  } catch (e) {
    const err = e as NodeJS.ErrnoException;
    if (err.code !== "EEXIST") throw err;
    let existing: string | null = null;
    try {
      existing = await readlink(linkPath);
    } catch {
      existing = null;
    }
    if (existing !== workspacesDir) {
      throw new Error(
        `workdir/workspaces exists but doesn't point to ${workspacesDir} (got ${existing ?? "non-link entry"})`,
      );
    }
  }

  const skills = opts.skills ?? [];
  if (skills.length > 0) {
    const skillsDir = join(workdir, "skills");
    await mkdir(skillsDir, { recursive: true });
    for (const s of skills) {
      await writeFile(join(skillsDir, `${s.name}.md`), s.content_md, "utf-8");
      // P3 carry-forward: associated text files materialize under
      // `<skills-dir>/<skill-name>/<file-name>`. Path safety is enforced
      // by the world's `file_name_is_valid` at upload time (no `..`, no
      // `/`, alphanumeric + dash + underscore + dot only), so a plain
      // join is safe here.
      const files = s.files ?? [];
      if (files.length > 0) {
        const skillSubdir = join(skillsDir, s.name);
        await mkdir(skillSubdir, { recursive: true });
        for (const f of files) {
          await writeFile(join(skillSubdir, f.name), f.content, "utf-8");
        }
      }
    }
  }

  const claudeMd = buildClaudeMd(opts, skills);
  await writeFile(join(workdir, "CLAUDE.md"), claudeMd, "utf-8");

  return workdir;
}

function buildClaudeMd(opts: PrepareWorkdirOpts, skills: SkillContent[]): string {
  const { agentId, taskId, startupId } = opts;
  const canonical = `workspaces/${startupId}/artifacts/${taskId}.md`;
  const lines = [
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
  ];
  if (skills.length > 0) {
    lines.push("## Available skills");
    lines.push("");
    lines.push("You have these reusable skills attached. Read them when relevant:");
    lines.push("");
    for (const s of skills) {
      lines.push(`- \`${s.name}\` → \`./skills/${s.name}.md\``);
    }
    lines.push("");
  }
  return lines.join("\n");
}
