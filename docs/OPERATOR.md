# Operator guide

You're sitting at the operator console (the React + Pixi 2D world).
This is how you actually run cliptown.

## Connecting

Open the console URL. The default dev token (`dev-token`) is wired
automatically when you connect to a local world. Against a deployed
instance, the token comes from `CLIPTOWN_OPERATOR_TOKEN` set at deploy
time — paste into the console's auth field, or set it as a query
param if your front-of-house allows.

## The town view

The 2D canvas shows agents (avatars) walking between rooms in their
respective towns. Each avatar's alpha reflects its **health bucket**
(P2.1):

- **Online** (alpha 1.0) — WS-connected, recent activity.
- **Recently lost** (alpha 0.7) — disconnected ≤ 5 min. Likely
  transient.
- **Offline** (alpha 0.4) — disconnected 5 min – 6 d.
- **About to GC** (alpha 0.3) — within last 24 h before the 7-day GC.

Click an avatar to inspect its current task, role, and attached
skills.

## Possessing a startup

Each startup has its own town. **Possess** a startup to enter its
world: your operator avatar drops into the lobby, the kanban + chat
panels filter to that startup, and `OperatorMove` / `OperatorDirective`
target avatars in that town.

Unpossess to return to the god-view.

## Managing tasks (Kanban)

Columns: `proposed`, `queued`, `in_progress`, `awaiting_review`,
`changes_requested`, `done`, `failed`, `escalated`.

Per-card actions:

- **Accept Proposal** (proposed → queued) — assigns an engineer.
- **Reject Proposal** (proposed → failed) — kills the task with a
  reason.
- **Force Accept** (awaiting_review → done) — bypass the manager
  review.
- **Force Fail** (any → failed) — emergency stop with a note.

Each card carries the `review_round` badge — bumps on every
`task_request_changes` cycle and caps at `max_review_rounds` (from
`cliptown.toml`, default 3). At cap, the next failed review escalates
to `failed`.

## Managing skills (P2.2)

The **SkillsPanel** in the sidebar lists skills for the currently-
possessed startup. Each skill row shows:

- **Name + size badge** — markdown content length.
- **Attached agents** — clickable chips with `×` to detach.
- **"Attach to…" dropdown** — filtered to unattached agents.

To **create a skill**, use the MCP tool (operators have an
admin-style "agent" account for skill authoring) or insert directly
into SQL:

```bash
docker compose exec world sqlite3 /data/cliptown.db \
  "INSERT INTO skills (id, startup_id, name, content_md, created_at, updated_at)
   VALUES (lower(hex(randomblob(16))), '<sid>', '<name>', '<markdown>',
           unixepoch(), unixepoch());"
```

Attach via the panel. Skills written into each spawn's
`<workdir>/skills/<name>.md` (P2.3 execenv).

A future PR will land inline content editor + attach UI for the
authoring path. Tracked in CHANGELOG known-limitations under M12 P2.2.

## Sending directives

The chat panel handles room-scoped chat + operator → agent
**directives**. Directives are sent via `OperatorDirective` and arrive
at the target agent's next session boot.

Body length: capped at 4096 chars (`MAX_BODY_LENGTH` in
`mcp_dispatch.rs`). Longer messages get rejected with
`body_too_long`.

## Budget controls

Per-startup budgets are set at create time. The console shows
`spent_usd / cap_usd` per startup. As spend climbs:

- **80%** — warning toast.
- **95%** — `no_new_task` flag. Existing tasks finish; no new spawns.
- **100%** — `pause` flag. All workers SIGTERM'd; can't restart
  without a cap raise.

Raise / lower the cap via `PATCH /api/startups/<id>`. Auto-resume from
100% is implicit: raising the cap above current spend prevents the
100% threshold from re-tripping on subsequent `report_budget` frames.

## System events

The system event feed (bottom of console) shows real-time alerts:
`budget_warn`, `budget_paused`, `task_escalated`, `agent_lost`, etc.
Severity is rendered with a color stripe (info / warn / alert /
critical).

## Inspecting state

For deeper inspection beyond the console:

```bash
# SQL spelunking.
docker compose exec world sqlite3 /data/cliptown.db

# Tail logs.
docker compose logs -f world

# Tool catalog.
curl -X POST http://localhost:8080/mcp \
  -H "Authorization: Bearer <agent_id>:<secret>" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

## Common ops recipes

### Drain a misbehaving startup

```bash
# Operator console: Force Fail all in_progress + queued tasks.
# Then via SQL:
docker compose exec world sqlite3 /data/cliptown.db \
  "UPDATE startups SET status = 'dissolved' WHERE id = '<sid>';"
# World loop's Cmd::ReleaseSuite cleans the in-memory state.
```

### Reset agent secret

```bash
# Edit your deploy's secrets:
fly secrets set CLIPTOWN_AGENT_SECRET_<agent_id>=<new>
# Worker reconnects with the new secret on its next supervisor respawn.
```

### Find a stuck task

```sql
SELECT id, title, status, assignee_agent_id, updated_at
FROM tasks WHERE status NOT IN ('done', 'failed')
ORDER BY updated_at;
```

### Garbage-collect old execenv workdirs

Per-task workdirs at `workspaces/<sid>/<tid>/workdir` accumulate over
time. `scripts/gc-execenv.sh` removes workdirs whose task is in a
terminal state (`done` / `failed` / `escalated`) AND was last updated
more than `--days N` (default 7) ago. Artifacts (`workspaces/<sid>/artifacts/`)
are preserved.

```bash
# Dry run first to see what would go.
scripts/gc-execenv.sh --dry-run

# Default — terminal & >7d.
scripts/gc-execenv.sh

# Aggressive — terminal & >1d, against a custom layout.
scripts/gc-execenv.sh --days 1 --db /data/cliptown.db --workspaces /workspaces
```

Safe to run while the world is up; the script opens SQLite read-only
and never touches active-state tasks.

## Where things live

- [`AGENT.md`](AGENT.md) — what cliptown looks like from inside an
  adapter-spawned CLI.
- [`DEPLOY.md`](DEPLOY.md) — Docker, Fly.io, secrets.
- [`../ARCHITECTURE.md`](../ARCHITECTURE.md) — system layout.
