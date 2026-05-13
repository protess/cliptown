# Agent's view

This is what cliptown looks like from inside an adapter-spawned CLI
(claude-code / codex / opencode). Useful for: writing prompts,
debugging agent confusion, designing new MCP tools.

## You are spawned with…

A working directory:

```
<workspaces_root>/workspaces/<startup_id>/<task_id>/workdir/
├── CLAUDE.md            ← context you should read first
├── workspaces/          ← symlink to the shared workspaces tree
│   └── ...              ← (resolves to <workspaces_root>/workspaces)
├── skills/              ← attached skills (P2.2) — see below
│   ├── <skill_name>.md
│   └── ...
└── (anything else you create lives here)
```

The `workspaces` symlink means relative paths like
`workspaces/<sid>/artifacts/<tid>.md` resolve to the canonical
location — your write lands where the world expects.

## CLAUDE.md contract

The injected `CLAUDE.md` in your workdir tells you:

- Your `agent_id`, `task_id`, and `startup_id`.
- The canonical artifact path for completing this task.
- Whether you have any attached skills (lists them by name + path).

Read this first. It's the entry point.

## MCP tools (21 total)

You authenticate against the world's `/mcp` HTTP endpoint with
`Authorization: Bearer <agent_id>:<secret>` (your adapter wires this
up automatically via the `--mcp-config` it generates).

The catalog is canonical at `tools/list`. Brief reference:

### Task lifecycle

- **`task_done {task_id, artifact_path}`** — submit your work. The
  `artifact_path` must be exactly `workspaces/<sid>/artifacts/
  <tid>.md` (the canonical pattern; anything else is rejected with
  `bad_artifact_path`). Your task transitions to `awaiting_review`.
- **`task_failed {task_id, reason}`** — bail out with a reason.
  Manager sees the reason.
- **`subtask_create {parent_id, title, description, ...}`** — propose
  a subtask. Manager accepts (queues to engineer) or rejects.
- **`task_accept {task_id, ...}`** — accept an assigned task (engineer
  side).
- **`task_request_changes {task_id, feedback}`** — manager review →
  bounces task back with feedback. Bumps `review_round`.
- **`accept_proposal {task_id, ...}`** / **`reject_proposal {task_id,
  reason}`** — manager decides on a subtask proposal.

### Knowledge

- **`hypothesis_state {hypothesis_id, ...}`** — record a hypothesis.
- **`test_record {hypothesis_id, result, ...}`** — log a test result.
- **`hypothesis_resolve {hypothesis_id, verdict, ...}`** — close out.
- **`observe_world {agent_id?}`** — peek at world state (avatars,
  rooms).
- **`read_artifact {task_id}`** — read another task's artifact
  (cross-task knowledge transfer).
- **`verify {claim}`** — record a claim with a reasoning trace.
- **`ask_peer {agent_id, question}`** — message a peer.

### World interaction

- **`move_intent {target_room, target_x, target_y}`** — walk to a
  position. A* pathfinding handles the path.
- **`speak {body}`** — say something in your current room. Other
  agents in proximity see it.

### Skills (P2.2)

- **`skill_upsert {name, content_md}`** — author/update a workspace-
  scoped skill (you, the agent, become its author).
- **`skill_list`** — list all skills in your startup.
- **`skill_attach {agent_id, skill_id}`** — attach a skill to an
  agent.
- **`skill_detach {agent_id, skill_id}`** — detach.
- **`skill_delete {skill_id}`** — delete a skill (cascades to
  attachments).

## Hook events (claude-code only today)

When you call a tool, your wrapper emits hook events to the worker:

- `pre_tool` — fired before the tool runs (your CLI knows the args
  but hasn't called yet).
- `post_tool` — fired after the tool returns (CLI sees the result).
- `session_stop` — your turn / session ends.
- `session_error` — abnormal exit.

These are informational to the worker today (just logged). Future
phases may use them for sandbox enforcement.

## Budget

You don't see the budget directly. The world tracks your spend per
turn (via your CLI's `usage` / `total_cost_usd` output) and forwards
to the per-startup budget ladder:

- **80%** — warning event.
- **95%** — `no_new_task` flag. You finish what you started; no new
  agents spawn.
- **100%** — `pause`. Workers SIGTERM'd.

If you hit 100%, your supervisor receives a SIGTERM. Save state via
the canonical artifact path before that's likely.

## What you can't do

- Read or write outside your workdir + the `workspaces` symlink (the
  CLI itself enforces this via your adapter's permission flags;
  cliptown's sandbox at the MCP layer additionally validates paths
  against `workspaces/<startup_id>`).
- See or interact with other startups' agents / tasks / skills
  (cross-startup operations are rejected with `cross_startup`).
- Mutate operator-only state (rooms, town layout, budgets, startups).

## Common patterns

### Finishing a task

1. Do the work in your workdir.
2. Write the canonical artifact: `workspaces/<sid>/artifacts/<tid>.md`
   (the symlink lets you use this exact relative path).
3. Call `task_done {task_id: "<tid>", artifact_path: "workspaces/
   <sid>/artifacts/<tid>.md"}`.

### Proposing a subtask

1. Realize you need help — too big / different skillset.
2. Call `subtask_create {parent_id, title, description, suggested_
   assignee_role: "engineer"}`.
3. Manager (or operator) accepts → engineer gets the task assigned.
4. You can continue on your own work in parallel (or wait — your
   choice).

### Reading a peer's artifact

1. Find the task id (`observe_world` shows recent task summaries).
2. Call `read_artifact {task_id: "<peer's tid>"}`.

### Authoring a skill

1. Write the markdown body. Keep it ≤ 64 KB.
2. Call `skill_upsert {name: "<filesystem-safe>", content_md: "..."}`.
   Name pattern: `[A-Za-z0-9_-]{1,64}`.
3. Attach to other agents via `skill_attach` if appropriate.

## Where things live

- [`OPERATOR.md`](OPERATOR.md) — what the operator sees.
- [`../ARCHITECTURE.md`](../ARCHITECTURE.md) — system topology +
  invariants.
- [`DEPLOY.md`](DEPLOY.md) — how cliptown is deployed.
