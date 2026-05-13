# P2.2 skills system (Phase 2 MVP) — design

**Date:** 2026-05-13
**Status:** draft — pending implementation
**Driver:** Phase 2 backlog last item (`docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md` § P2.2). cliptown agents today see only `task.title` + `task.description`. No way to compound reusable capability — every new agent session reinvents the wheel. Skills give "this agent knows how to do X" a concrete artifact form: a named markdown body attached to one or more agents, written into the agent's per-task workdir at spawn time so the CLI's filesystem context includes it.

P2.3 (per-task execenv) shipped in PR #41 — workdir + symlink + CLAUDE.md infrastructure exists. P2.2 plugs into that. Without P2.3 there'd be nowhere to write skill files.

## Goals (this PR — MVP)

- Operators can author **markdown skills** per startup (workspace-scoped).
- Operators can **attach** skills to specific agents (many-to-many).
- At `--real` adapter spawn, worker fetches the agent's attached skills and writes each as `<workdir>/skills/<skill_name>.md`.
- The injected `CLAUDE.md` lists attached skill names so the CLI knows the files are available.
- World exposes 5 MCP tools (`skill_upsert`, `skill_list`, `skill_attach`, `skill_detach`, `skill_delete`) and 1 HTTP endpoint (`GET /api/agents/:agent_id/skills`).
- Smoke verifies an attached skill lands at `<workdir>/skills/<name>.md` on disk.

## Non-goals (explicit — follow-up PRs)

- **Frontend skill management UI.** Operators use MCP tools or direct SQL for the MVP.
- **`skill_changed` ConsoleOutbound broadcasts.** Lazy fetch at spawn is the contract; live skill edits don't affect in-flight tasks.
- **Global (non-workspace-scoped) skills.** Skills live under `startups(id)` only.
- **File attachments beyond markdown.** Each skill is a single markdown body. Future PR adds an attached files array if needed.
- **Agent templates / skill bundles.** Direct attachment via `agent_skills` only.
- **Versioning / skill history.** Upsert is mutable; latest content wins. Future PR adds a `skill_revisions` log if needed.

## Schema (migration `0002_skills.sql`)

```sql
CREATE TABLE skills (
  id TEXT PRIMARY KEY,
  startup_id TEXT NOT NULL REFERENCES startups(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  content_md TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (startup_id, name)
);

CREATE INDEX idx_skills_startup ON skills(startup_id);

CREATE TABLE agent_skills (
  agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
  skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
  attached_at INTEGER NOT NULL,
  PRIMARY KEY (agent_id, skill_id)
);

CREATE INDEX idx_agent_skills_agent ON agent_skills(agent_id);
```

Schema invariants:
- `(startup_id, name)` unique — a startup has at most one skill per name. Upsert by name.
- `agent_skills` has no `startup_id` column — derived through `agents.startup_id`. Cross-startup attaches are rejected in the handler.

## MCP tools (5 new — total moves 16 → 21)

All handlers verify `caller.startup_id` matches the skill / agent's startup. Cross-startup operations return `{type:"error",reason:"cross_startup"}`.

### `skill_upsert {name, content_md} → {id, created}`

- Caller's startup_id is used as scope.
- Insert if `(startup_id, name)` doesn't exist; update `content_md` + `updated_at` if it does.
- `name` constraints: 1–64 chars, `[A-Za-z0-9_-]` only (avoids filesystem-unsafe chars; matches multica). Rejects malformed names with `bad_skill_name`.
- `content_md` constraints: ≤ 64 KB (defensive cap — prevents prompt blowup; covers realistic skill content).
- Returns: `{id: <uuid>, created: bool}`.

### `skill_list {} → {skills: [{id, name, updated_at, len}]}`

- Returns all skills in caller's startup. Excludes `content_md` (operators page through listing then fetch one by id via SQL for v1 — UI will add a per-skill fetch tool).
- `len` is `length(content_md)` for quick triage.

### `skill_attach {agent_id, skill_id} → {ok: true}`

- Verifies `agent_id` and `skill_id` both belong to caller's startup.
- Insert into `agent_skills`. `ON CONFLICT DO NOTHING` — already-attached is OK (idempotent).

### `skill_detach {agent_id, skill_id} → {ok: true}`

- Same cross-startup checks.
- `DELETE FROM agent_skills WHERE ...`. Not-attached is OK (idempotent).

### `skill_delete {skill_id} → {ok: true}`

- Verifies skill belongs to caller's startup.
- `DELETE FROM skills WHERE id = ?`. `agent_skills` rows cascade.

## HTTP API endpoint (1 new)

### `GET /api/agents/:agent_id/skills`

- Bearer auth via `Authorization: Bearer <agent_id>:<secret>` — same scheme as MCP HTTP. The caller in the bearer must equal `:agent_id` in the path (an agent can only fetch its own skills).
- Returns `{skills: [{name, content_md}]}` — the agent's attached skills with full markdown content.
- Response shape is what `prepareWorkdir` needs verbatim.
- No PUT/POST — skill mutations go through MCP tools.

## Worker integration

`packages/worker/src/execenv.ts::PrepareWorkdirOpts` gains an optional `skills`:

```ts
export interface SkillContent {
  name: string;
  content_md: string;
}

export interface PrepareWorkdirOpts {
  workspacesRoot: string;
  startupId: string;
  taskId: string;
  agentId: string;
  skills?: SkillContent[];
}
```

When `skills` is non-empty, `prepareWorkdir`:
1. `mkdir -p <workdir>/skills/`.
2. For each `{name, content_md}` write `<workdir>/skills/<name>.md`.
3. Add a "## Available skills" section to `CLAUDE.md` listing each attached skill's name + relative path. Example:
   ```markdown
   ## Available skills

   You have these reusable skills attached. Read them when relevant:

   - `<name>` → `./skills/<name>.md`
   ```
4. If `skills` is omitted or empty, no `skills/` dir is created and no section is added to CLAUDE.md (preserves current behavior for unattached agents).

`packages/worker/src/skills_client.ts` *(new module)*:
- `fetchSkillsForAgent(worldHttpBase, agentId, secret) → Promise<SkillContent[]>` — single `fetch` call, returns `[]` on 404 or empty list, throws on other errors.

`packages/worker/src/main.ts` `--real` branch:
1. Right before `prepareWorkdir`, call `fetchSkillsForAgent(mcpWorldUrl, args.agentId, args.secret)`.
2. Pass the result via `skills:` to prepareWorkdir.

## Frontend impact

None for v1. `AvatarVM` / `WorldState` shape unchanged. Future PR adds a `skill_changed` ConsoleOutbound and a skills page; this PR doesn't touch frontend.

## Tests

### Rust unit (`crates/world/src/skills.rs::tests`)

8 tests on the DAO layer (in-memory SQLite via `TestCtx`):

1. `upsert_inserts_new_skill_with_id`
2. `upsert_updates_existing_skill_by_name`
3. `upsert_rejects_bad_name_chars`
4. `upsert_rejects_oversize_content`
5. `attach_is_idempotent`
6. `attach_rejects_cross_startup_agent_or_skill`
7. `delete_cascades_to_agent_skills`
8. `list_returns_metadata_only_no_content`

### Rust integration (`crates/world/tests/skills_integration.rs`)

3 tests booting `loop_::spawn` + `mcp_dispatch::dispatch`:

1. `mcp_skill_upsert_then_list_round_trip`
2. `mcp_skill_attach_then_get_via_http`
3. `mcp_skill_delete_cascades_to_agent_skills`

### Rust integration (`crates/world/tests/api_skills.rs`)

3 tests on the HTTP endpoint:

1. `get_agent_skills_returns_attached_with_content`
2. `get_agent_skills_rejects_wrong_bearer`
3. `get_agent_skills_returns_empty_list_for_unattached_agent`

### Worker unit (`packages/worker/test/execenv.test.ts`) — additions

2 tests on the extended `prepareWorkdir`:

1. `writes attached skills as <workdir>/skills/<name>.md`
2. `omits skills section when skills is empty`

### Worker unit (`packages/worker/test/skills_client.test.ts`) — new

2 tests with an in-process http server:

1. `fetchSkillsForAgent returns parsed list on 200`
2. `fetchSkillsForAgent returns [] on 404`

### Smoke (`scripts/smoke-real-llm.sh`)

Adds a step before the worker spawn that:
1. Creates a skill via SQL or MCP call: `INSERT INTO skills (id, startup_id, name, content_md, created_at, updated_at) VALUES (...)` with a fixture name `smoke-skill-deploy`.
2. Attaches it to the engineer agent.
3. After worker spawn completes, verifies:
   - `<workdir>/skills/smoke-skill-deploy.md` exists and contains the fixture content.
   - `<workdir>/CLAUDE.md` contains `smoke-skill-deploy` in the "## Available skills" section.

## Migration / risk

### Compatibility

- Existing `prepareWorkdir` callers that don't pass `skills` see exactly the previous behavior — no `skills/` dir, no CLAUDE.md section.
- Existing worker callers that don't pass attached skills (mock mode, contract tests, fixtures) are unaffected — `fetchSkillsForAgent` only runs in `--real`, and even there returns `[]` for empty attachment.
- Migration `0002_skills.sql` is additive — no existing tables touched. The migrations runner (storage.rs) already applies all files alphabetically.

### Risk

- **Skill name filesystem-safety.** Names are constrained to `[A-Za-z0-9_-]{1,64}` so the path `<workdir>/skills/<name>.md` is safe. Reject malformed at upsert time.
- **Content size.** 64KB cap prevents pathological content. Reasonable headroom — multica's largest skills are ~10KB.
- **Race on attach.** Two operators attaching the same skill concurrently: `ON CONFLICT DO NOTHING` keeps it idempotent.
- **Symlink resolution.** Skills/ is a regular directory inside workdir (no symlinks involved). `<workdir>/workspaces` symlink from P2.3 is untouched.
- **Mock mode.** `fetchSkillsForAgent` is only called in `--real`. Mock + contract tests stay green.

## Definition of done

- Migration applies cleanly; `cargo test -p cliptown-world` passes.
- 5 MCP tools registered + dispatched + handled; tool catalog count 16 → 21.
- HTTP endpoint mounted at `/api/agents/:id/skills` with bearer auth.
- Worker fetches skills in `--real`, writes them to `<workdir>/skills/`, CLAUDE.md mentions them.
- Test counts:
  - Rust: 231 + 8 (skills.rs::tests) + 3 (skills_integration) + 3 (api_skills) = 245
  - Worker: 70 + 2 (execenv extension) + 2 (skills_client) = 74
- Adapters / frontend / bench: unchanged (35 / 14 / ok=true).
- Smoke: skill upsert + attach + spawn → file lands at `<workdir>/skills/<name>.md`; CLAUDE.md contains the skill name.
- CHANGELOG M12 P2.2 section + TODOS Completed entry.
- Known limitations carried forward in CHANGELOG: no UI / no broadcasts / no global skills / no file attachments / no versioning.
