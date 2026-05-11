/**
 * M9.10 B — real-LLM E2E runner (machine-readable, maintainer-run).
 *
 * Same end-to-end exercise as `scripts/smoke-real-llm.sh` (sub-task A3),
 * but with structured JSON summary on stdout instead of colored human
 * output. Non-zero exit on any failure. Was originally written for sub-task
 * C's CI workflow; the workflow was removed (cliptown is open source, we
 * don't require contributors to provide Anthropic API keys in CI secrets),
 * so this runner now lives as a maintainer-run local artifact whose JSON
 * output is the proof attached to ship-gate § 11.9.
 *
 * Pre-requisites the maintainer must satisfy:
 *   - `claude` CLI on PATH (`npm install -g @anthropic-ai/claude-code`)
 *   - `ANTHROPIC_API_KEY` exported (not preflight-validated here — the CLI
 *     surfaces its own auth error when it tries to reach api.anthropic.com)
 *   - `cargo` + `pnpm` + `sqlite3` + `curl` on PATH
 *
 * Tunable env:
 *   - `E2E_BUDGET_CAP_USD` (default 0.50)
 *   - `E2E_WORLD_BIND`      (default 127.0.0.1:8080)
 *   - `E2E_OPERATOR_TOKEN`  (default dev-token)
 *   - `E2E_AGENT_SECRET`    (default dev-secret)
 *   - `E2E_KEEP_TMP=1`      preserve tmpdir on exit (CI artifact retention)
 *
 * Exit:
 *   - 0 on full success.
 *   - 1 on any verification or process failure; the summary JSON's
 *     `ok=false` + `step` fields tell the workflow what broke.
 */

import { spawn, type ChildProcess } from "node:child_process";
import { mkdir, copyFile, stat, writeFile, rm } from "node:fs/promises";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

interface RunCtx {
  repoRoot: string;
  smokeDir: string;
  budgetCapUsd: number;
  worldBind: string;
  operatorToken: string;
  agentSecret: string;
  taskId: string;
  worldProc: ChildProcess | null;
  startedAt: number;
}

interface Summary {
  ok: boolean;
  duration_ms: number;
  budget_cap_usd: number;
  step?: string;
  error?: string;
  startup_id?: string;
  engineer_id?: string;
  task_id?: string;
  artifact_bytes?: number;
  task_status?: string;
  artifact_path?: string;
  budget_spent_usd?: number;
  smoke_dir?: string;
}

class StepError extends Error {
  constructor(public step: string, message: string, public details?: unknown) {
    super(message);
  }
}

function envOr(name: string, fallback: string): string {
  const v = process.env[name];
  return v && v.length > 0 ? v : fallback;
}

function buildCtx(): RunCtx {
  const repoRoot = resolve(new URL(".", import.meta.url).pathname, "..", "..", "..");
  const smokeDir = mkdtempSync(join(tmpdir(), "cliptown-e2e-"));
  return {
    repoRoot,
    smokeDir,
    budgetCapUsd: Number.parseFloat(envOr("E2E_BUDGET_CAP_USD", "0.50")),
    worldBind: envOr("E2E_WORLD_BIND", "127.0.0.1:8080"),
    operatorToken: envOr("E2E_OPERATOR_TOKEN", "dev-token"),
    agentSecret: envOr("E2E_AGENT_SECRET", "dev-secret"),
    taskId: "smoke-haiku",
    worldProc: null,
    startedAt: Date.now(),
  };
}

// ── child-process helpers ──────────────────────────────────────────────────

/**
 * Run a child to completion, capturing stdout+stderr. Throws StepError on
 * non-zero exit. Stderr is merged into the error details so CI logs see the
 * actual failure cause.
 */
async function run(
  step: string,
  cmd: string,
  args: string[],
  opts: { cwd?: string; env?: NodeJS.ProcessEnv } = {},
): Promise<{ stdout: string; stderr: string }> {
  return new Promise((resolveRun, reject) => {
    const child = spawn(cmd, args, {
      cwd: opts.cwd ?? process.cwd(),
      env: { ...process.env, ...opts.env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (b: Buffer) => { stdout += b.toString("utf-8"); });
    child.stderr.on("data", (b: Buffer) => { stderr += b.toString("utf-8"); });
    child.on("error", (e) => reject(new StepError(step, e.message)));
    child.on("close", (code) => {
      if (code === 0) {
        resolveRun({ stdout, stderr });
      } else {
        reject(new StepError(step, `${cmd} exited rc=${code}`, { stdout, stderr }));
      }
    });
  });
}

// ── steps ──────────────────────────────────────────────────────────────────

async function preflight(): Promise<void> {
  for (const tool of ["claude", "cargo", "pnpm", "sqlite3", "curl"]) {
    try {
      await run("preflight", "command", ["-v", tool]);
    } catch {
      throw new StepError("preflight", `missing required tool: ${tool}`);
    }
  }
  // Note: ANTHROPIC_API_KEY presence is NOT validated here. If it's missing
  // or wrong, the `claude` CLI emits a clearer error (unset vs invalid vs
  // rate-limited) when it actually tries to reach api.anthropic.com — the
  // summary then surfaces it under `step: "spawn_worker"`.
  await run("preflight", "claude", ["--version"]);
}

async function buildWorld(ctx: RunCtx): Promise<void> {
  await run("build_world", "cargo", ["build", "--release", "-p", "cliptown-world"], {
    cwd: ctx.repoRoot,
  });
}

async function bootWorld(ctx: RunCtx): Promise<void> {
  await copyFile(join(ctx.repoRoot, "cliptown.toml"), join(ctx.smokeDir, "cliptown.toml"));
  const binPath = join(ctx.repoRoot, "target", "release", "cliptown-world");
  const child = spawn(binPath, [], {
    cwd: ctx.smokeDir,
    env: {
      ...process.env,
      CLIPTOWN_DB: join(ctx.smokeDir, "cliptown.db"),
      CLIPTOWN_ADDR: ctx.worldBind,
      // Tell /api/startups to use our agentSecret instead of generating a
      // random one. The worker we spawn below authenticates with this value;
      // without the override the world's auto-generated secret would reject
      // our hello with auth_error. See `crates/world/src/api_startups.rs`.
      CLIPTOWN_TEST_FIXED_AGENT_SECRET: ctx.agentSecret,
      // Skip the world's auto-spawn of worker children. The smoke spawns
      // its single worker explicitly below; letting the supervisor try with
      // the relative `packages/worker/bin/worker` path from this tmpdir
      // just emits a few rounds of `spawn_agent failed` warnings.
      CLIPTOWN_TEST_DISABLE_SUPERVISOR: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  ctx.worldProc = child;
  // Tee world stdio into a log file so post-mortem investigation is possible.
  const logPath = join(ctx.smokeDir, "world.log");
  const logStream = (await import("node:fs")).createWriteStream(logPath);
  child.stdout?.pipe(logStream);
  child.stderr?.pipe(logStream);

  // Poll /health up to 30s (release-build cold-start is ~1s in practice).
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new StepError(
        "boot_world",
        `world exited rc=${child.exitCode} before /health responded; see ${logPath}`,
      );
    }
    try {
      const resp = await fetch(`http://${ctx.worldBind}/health`);
      if (resp.ok) return;
    } catch {
      /* not ready yet */
    }
    await sleep(500);
  }
  throw new StepError("boot_world", `world /health never responded; log at ${logPath}`);
}

async function createStartup(
  ctx: RunCtx,
): Promise<{ startupId: string; engineerId: string; founderId: string }> {
  const resp = await fetch(`http://${ctx.worldBind}/api/startups`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      Authorization: `Bearer ${ctx.operatorToken}`,
    },
    body: JSON.stringify({
      name: "e2e",
      goal_text: "haiku",
      budget_cap_usd: ctx.budgetCapUsd,
      backends: { founder: "claude_code", engineer: "claude_code", designer: "claude_code" },
    }),
  });
  if (!resp.ok) {
    throw new StepError("create_startup", `POST /api/startups rc=${resp.status}`, {
      body: await resp.text(),
    });
  }
  const body = (await resp.json()) as {
    id: string;
    agents: Array<{ id: string; role: string }>;
  };
  const engineer = body.agents.find((a) => a.role === "engineer");
  const founder = body.agents.find((a) => a.role === "founder");
  if (!engineer || !founder) {
    throw new StepError("create_startup", "response missing engineer/founder agent", body);
  }
  return { startupId: body.id, engineerId: engineer.id, founderId: founder.id };
}

async function seedTasks(
  ctx: RunCtx,
  startupId: string,
  engineerId: string,
  founderId: string,
): Promise<void> {
  // mcp_dispatch::handle_task_done's subtask_done fanout expects a manager
  // chain; create the parent task assigned to founder so the engineer's
  // task_done has somewhere to fan to.
  const sql = `
INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at)
VALUES ('T-parent', '${startupId}', NULL, 'parent', 'd', 'in_progress', '${founderId}', unixepoch(), unixepoch());
INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at)
VALUES ('${ctx.taskId}', '${startupId}', 'T-parent', 'Write a haiku', 'd', 'in_progress', '${engineerId}', unixepoch(), unixepoch());
`;
  await run("seed_tasks", "sqlite3", [join(ctx.smokeDir, "cliptown.db"), sql]);
}

function buildPrompt(ctx: RunCtx, startupId: string): string {
  const artifactRel = `workspaces/${startupId}/artifacts/${ctx.taskId}.md`;
  return [
    "You are an engineer in a simulated environment. You have ONE task. Follow these steps in order:",
    "",
    "1. Use the Write tool to create a file at this EXACT relative path:",
    `     ${artifactRel}`,
    "   The file content must be a three-line haiku about clipboards. The file does not exist yet.",
    "",
    // Single quotes around the tool name (not backticks): when this string
    // transits any layer that re-evaluates as a shell command line, backtick
    // segments get command-substituted. Single quotes are markdown-equivalent
    // emphasis as far as the model is concerned.
    "2. After the file is written, call the MCP tool named 'mcp__cliptown__task_done' with arguments:",
    `     task_id: "${ctx.taskId}"`,
    `     artifact_path: "${artifactRel}"`,
    "",
    "Do not use any other tools. Do not edit or re-read the file. Stop immediately after task_done returns.",
  ].join("\n");
}

async function spawnWorker(
  ctx: RunCtx,
  startupId: string,
  engineerId: string,
): Promise<void> {
  // Pre-create the workspace dir so claude's Write tool + world's
  // sandbox::resolve agree.
  await mkdir(join(ctx.smokeDir, "workspaces", startupId, "artifacts"), {
    recursive: true,
  });
  const prompt = buildPrompt(ctx, startupId);
  // Write the prompt to a file so we can attach it to the JSON summary if
  // verification later wants to inspect what the engineer was asked to do.
  await writeFile(join(ctx.smokeDir, "prompt.txt"), prompt, "utf-8");

  // Three reasons we bypass `pnpm <pkg> start --` and call tsx directly:
  //   1. pnpm needs to run from inside the workspace to find @cliptown/worker
  //      via pnpm-workspace.yaml; running from smokeDir silently no-ops with
  //      "No projects found in <cwd>" and rc=0.
  //   2. pnpm forwards a literal "--" as a positional arg to the script,
  //      which then trips the worker's `allowPositionals: false` parseArgs.
  //   3. pnpm re-shells the script command, so backticks inside the prompt
  //      arg get command-substituted.
  // `pnpm -F @cliptown/worker exec tsx ./src/main.ts <args>` resolves tsx
  // from the worker's own node_modules and passes argv directly without
  // further shell evaluation.
  await run(
    "spawn_worker",
    "pnpm",
    [
      "-F",
      "@cliptown/worker",
      "exec",
      "tsx",
      "./src/main.ts",
      "--world-url",
      `ws://${ctx.worldBind}/ws/worker`,
      "--agent-id",
      engineerId,
      "--startup-id",
      startupId,
      "--secret",
      ctx.agentSecret,
      "--backend",
      "claude_code",
      "--workspace",
      ctx.smokeDir,
      "--real",
      "--prompt",
      prompt,
    ],
    { cwd: ctx.repoRoot },
  );
}

async function verify(
  ctx: RunCtx,
  startupId: string,
): Promise<{
  artifact_bytes: number;
  task_status: string;
  artifact_path: string;
  budget_spent_usd: number;
}> {
  const artifactRel = `workspaces/${startupId}/artifacts/${ctx.taskId}.md`;
  const artifactAbs = join(ctx.smokeDir, artifactRel);

  let st;
  try {
    st = await stat(artifactAbs);
  } catch {
    throw new StepError("verify_disk", `artifact missing: ${artifactAbs}`);
  }
  if (st.size === 0) {
    throw new StepError("verify_disk", `artifact empty: ${artifactAbs}`);
  }

  const taskRowQ = `SELECT status||'|'||COALESCE(artifact_path,'') FROM tasks WHERE id = '${ctx.taskId}';`;
  const { stdout: taskRowOut } = await run("verify_sql_task", "sqlite3", [
    join(ctx.smokeDir, "cliptown.db"),
    taskRowQ,
  ]);
  const [status, pathFromDb] = taskRowOut.trim().split("|");
  if (status !== "awaiting_review") {
    throw new StepError(
      "verify_sql_task",
      `expected status=awaiting_review, got '${status}'`,
      { stdout: taskRowOut },
    );
  }
  if (pathFromDb !== artifactRel) {
    throw new StepError(
      "verify_sql_task",
      `expected artifact_path=${artifactRel}, got '${pathFromDb}'`,
    );
  }

  const budgetQ = `SELECT budget_spent_usd FROM startups WHERE id = '${startupId}';`;
  const { stdout: budgetOut } = await run("verify_sql_budget", "sqlite3", [
    join(ctx.smokeDir, "cliptown.db"),
    budgetQ,
  ]);
  const spent = Number.parseFloat(budgetOut.trim());
  if (!Number.isFinite(spent)) {
    throw new StepError("verify_sql_budget", `unparseable spend '${budgetOut}'`);
  }
  if (spent > ctx.budgetCapUsd) {
    throw new StepError(
      "verify_sql_budget",
      `spend ${spent} exceeded cap ${ctx.budgetCapUsd}`,
    );
  }

  return {
    artifact_bytes: st.size,
    task_status: status,
    artifact_path: pathFromDb,
    budget_spent_usd: spent,
  };
}

async function cleanup(ctx: RunCtx): Promise<void> {
  if (ctx.worldProc && ctx.worldProc.exitCode === null) {
    ctx.worldProc.kill("SIGTERM");
    // Best-effort short wait — CI doesn't care if world lingers a moment.
    await new Promise<void>((r) => {
      const t = setTimeout(() => r(), 2000);
      ctx.worldProc!.once("close", () => {
        clearTimeout(t);
        r();
      });
    });
  }
  if (process.env.E2E_KEEP_TMP !== "1") {
    await rm(ctx.smokeDir, { recursive: true, force: true });
  }
}

// ── orchestration ─────────────────────────────────────────────────────────

async function main(): Promise<void> {
  const ctx = buildCtx();
  const summary: Summary = {
    ok: false,
    duration_ms: 0,
    budget_cap_usd: ctx.budgetCapUsd,
    smoke_dir: ctx.smokeDir,
    task_id: ctx.taskId,
  };
  let startupId: string | undefined;
  let engineerId: string | undefined;
  try {
    await preflight();
    await buildWorld(ctx);
    await bootWorld(ctx);
    const ids = await createStartup(ctx);
    startupId = ids.startupId;
    engineerId = ids.engineerId;
    summary.startup_id = startupId;
    summary.engineer_id = engineerId;
    await seedTasks(ctx, ids.startupId, ids.engineerId, ids.founderId);
    await spawnWorker(ctx, ids.startupId, ids.engineerId);
    const v = await verify(ctx, ids.startupId);
    Object.assign(summary, v);
    summary.ok = true;
  } catch (e) {
    if (e instanceof StepError) {
      summary.step = e.step;
      summary.error = e.message;
      if (e.details !== undefined) {
        (summary as unknown as Record<string, unknown>).details = e.details;
      }
    } else {
      summary.step = "unknown";
      summary.error = e instanceof Error ? e.message : String(e);
    }
  } finally {
    summary.duration_ms = Date.now() - ctx.startedAt;
    await cleanup(ctx);
    // Single JSON line on stdout — the workflow surfaces this directly.
    process.stdout.write(JSON.stringify(summary) + "\n");
    process.exit(summary.ok ? 0 : 1);
  }
}

main().catch((e) => {
  // Defense in depth: if main itself throws, still emit a JSON line.
  process.stdout.write(
    JSON.stringify({
      ok: false,
      step: "main",
      error: e instanceof Error ? e.message : String(e),
      duration_ms: 0,
      budget_cap_usd: 0,
    }) + "\n",
  );
  process.exit(1);
});
