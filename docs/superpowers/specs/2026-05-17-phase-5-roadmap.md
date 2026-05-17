# Phase 5 — roadmap brainstorm

**Date:** 2026-05-17
**Status:** brainstorm — not a binding spec, just direction.
**Driver:** Phase 4 sealed (#76 → #84). 4 themes shipped (F1, E1,
E2, E3) + 5 Theme G UX slices. The interesting question for
Phase 5 is what comes after "rich multi-agent coordination on a
single dev box."

## Where we are

- **Phase 4 themes** (#76–#79): local-LLM smoke (F1), peer review
  (E1), blocking deps + deadlines (E2), async work-stealing (E3).
- **Theme G** (#80–#84): SystemEvent toasts + readable marquee,
  admin toggles for peer-reviewer + auto-steal, Kanban
  blocked/deadline badges + steal flash, SkillsPanel revision
  history, HistoryModal filtering + OperatorsPanel grouping.
  Drained the polish bucket from Phase 4 roadmap.
- **Deferred from Phase 4:** F2 federation (XL, "pending real
  demand"). Still deferred — see "What this doc is NOT" below.
- **Operator console state today:** one operator at a time
  works. Two operators on the same cliptown can connect but
  don't see each other, can clobber each other's destructive
  actions, and read audit history as opaque `op_abc12def…` ids.
- **Deploy state today:** `pnpm dev` + a Rust binary launched
  by hand. DEPLOY.md documents fly.io+vercel; no
  `Dockerfile` / `docker-compose.yml` / CI deploy workflow.

Cliptown's coordination loop is solid for one operator + N
agents. Phase 5's question: what does it take to put 2-5
operators on the same cliptown and let them run it as a small
team's coordination tool?

**Phase 5 framing:** small-team coordination tool. Not SaaS,
not corp-SSO, not federated across instances. One shared
cliptown, 2-5 operators with bearer tokens, self-hosted via
docker-compose.

## Phase 5 candidate themes

Sorted by sequencing dependency (later themes lean on earlier
ones for shared infrastructure / display patterns).

### Theme P5.A — Operator presence (recommended first)

**Why:** Two operators on the same world today don't see each
other. No way to know if someone else is editing a startup,
mid-typing a directive, possessing the same agent. Table-stakes
for small-team mode.

**Scope:**
- New `ConsoleOutbound::OperatorPresence { v, presences: [...] }`
  broadcast on connect/disconnect/focus-change + 30s heartbeat
  tick. Each presence entry: `operator_id`, `name`, `role`,
  `focused_startup_id?`, `last_seen_at`.
- New `ConsoleInbound::PresenceHeartbeat { v, focused_startup_id? }`
  the frontend emits on startup-click and on `visibilitychange`
  (window focus/blur).
- Server-side in-memory `HashMap<operator_id, PresenceState>` with
  90s TTL (drop after 3 missed heartbeats). Cleanup task in the
  console WS broker.
- Sidebar shows a small presence dot/avatar per startup row
  indicating which operator(s) are focused there.
- TopBar shows collapsed avatars of all online operators with
  hover-tooltip names.

**Size:** S–M (one PR).

**Closes:** "I don't know my teammate is also looking at this"
gap. Foundation for B and C displays.

### Theme P5.B — Per-operator audit visibility

**Why:** Audit trails already store `actor_operator_id`, but the
UI shows opaque ids. Once 2-5 humans are touching the same
world, "who did this?" is the most common question.

**Scope:**
- Server-side: when emitting `ConsoleOutbound::Directive` and
  `ConsoleOutbound::SystemEvent` payloads that reference an
  operator id, join against `operators.name` and include a
  `display_name` field alongside the id.
- Frontend: HistoryModal renders the resolved name with an
  `op:` prefix; ChatPanel renders operator-sourced directives
  with the operator name instead of the sentinel "operator".
- Deterministic color hash per `operator_id` (8-hue palette
  like the existing startup hue) so the same operator always
  reads visually consistent across panels.

**Size:** S (one PR).

**Closes:** "Who did this?" pain in audit/history. Reuses the
operator id↔name lookup primitive from A.

### Theme P5.C — Soft-locks on destructive actions

**Why:** With two operators, the worst case is both clicking
"Force-Accept T1" at the same time, or one revoking the
other's token mid-action. Even if rare, the UX should make
clobbering visible and yieldable.

**Scope:**
- New SQL table `action_locks` with `id`, `lock_key`,
  `operator_id`, `acquired_at`, `expires_at`. Sample
  `lock_key`: `task:T1:force_accept`, `operator:op_xyz:revoke`,
  `startup:s1:delete`.
- Destructive ConsoleInbound paths (`OperatorForceAccept`,
  `OperatorForceFail`, `OperatorRevoke`, future
  `OperatorStartupDelete`): try-insert a lock row with 30s
  TTL on the lock_key. Conflict → reply
  `{type:"error", reason:"locked_by", operator:<name>, expires_at:<ts>}`.
- New `ConsoleOutbound::ActionLocked { lock_key, operator_id,
  operator_name, expires_at }` + `ActionUnlocked { lock_key }`
  broadcasts so any other connected client refreshes UI
  state without polling.
- Frontend pre-fetches lock state on snapshot connect; renders
  affected buttons disabled with "locked by Alice — 25s" text.

**Size:** M (one PR). New table + migration + new
ConsoleInbound dispatch + new ConsoleOutbound variant +
frontend state.

**Closes:** dueling-operator footgun on force-accept /
force-fail / revoke / delete. Leans on A's presence rendering
and B's name resolution.

### Theme P5.D — Observability + alerts

**Why:** Prometheus `/metrics` exists (P3 Theme D era), but
no Grafana dashboards or alert rules live in the repo. Once
2+ humans care, you need a "what's broken?" surface that
doesn't require log-tailing.

**Scope:**
- `docs/observability/grafana/cliptown-overview.json` —
  importable dashboard JSON with panels for:
  tick rate (Hz), dispatch latency p50/p95, MCP call latency
  by tool, queued/in-progress task counts per startup,
  escalation rate, budget spend %, system_event rate by
  kind, agent health buckets count.
- `docs/observability/alerts/cliptown.yml` — Alertmanager-
  compatible rule file:
  - tick rate < 0.5 Hz sustained 30s
  - escalation rate > 1/min sustained 60s
  - any startup with `budget_spent_pct > 95` sustained 60s
  - any agent in `recently_lost` or `offline` health bucket
  - SQL error counter increases
- README addition: "How to wire this up" with a one-paragraph
  walkthrough using the docker-compose `observability`
  profile (lands in P5.E).
- The compose `observability` profile mounts the dashboard
  JSON and alert YAML as configmaps so a `docker compose
  --profile observability up -d` brings up Grafana
  pre-provisioned.

**Size:** M (heavy on JSON + YAML, light on Rust).

**Closes:** "what's broken?" without log-tailing. D's
dashboard+alert artifacts ship as files; E's compose file
then mounts them so `docker compose --profile observability
up -d` brings them online. D is useful as documentation in
isolation; full runtime value is realized once E lands.

### Theme P5.E — Deploy pipeline (Docker)

**Why:** DEPLOY.md documents fly.io+vercel but no `Dockerfile`,
`docker-compose.yml`, or CI deploy workflow lives in the repo.
Hosting a team-shared cliptown requires hand-following the
doc. Docker self-host fits the "small team, self-managed"
target far better than a managed-SaaS deploy.

**Scope:**
- World `Dockerfile`: multi-stage with cargo chef (build deps
  cache) → release build → minimal runtime
  (`gcr.io/distroless/cc-debian12` or `debian:slim`). Volume
  mount for the SQLite DB + `workspaces/` dir. `EXPOSE 8080`,
  `HEALTHCHECK` against `/health`.
- Frontend `Dockerfile`: `pnpm build` → static assets served
  by nginx; nginx config ports the Vite dev-proxy rewrites
  (`/api/*` and `/ws/*` → `world:8080`).
- `docker-compose.yml` at repo root: services `world` +
  `frontend` + named volumes for SQLite and workspaces.
  Compose profiles: default (`world` + `frontend` only),
  `observability` (adds `prometheus` + `grafana` with the
  P5.D configs mounted).
- `.github/workflows/ci.yml`: cargo fmt + clippy + cargo
  test + pnpm tsc + frontend playwright e2e + docker build
  smoke (verify both images build clean).
- `.github/workflows/release.yml`: tag-triggered docker
  buildx multi-arch push to GHCR
  (`ghcr.io/<org>/cliptown-world` +
  `ghcr.io/<org>/cliptown-frontend`).
- DEPLOY.md rewrite around `docker compose up -d` + "pull the
  latest tag from GHCR." fly/vercel references drop out.

**Size:** M–L (one PR, but heavy on first-deploy debugging
across two images).

**Closes:** "I have to hand-follow the doc" pain. Provides
the orchestration shell that D's observability profile plugs
into.

### Theme P5.F — Backup/restore drill

**Why:** SQLite WAL is great for live perf but doesn't
protect against (a) volume corruption on the docker host,
(b) catastrophic schema migration. Once 2+ humans depend on
the world state, lost state is a phase-killer.

**Scope:**
- New `crates/world/src/backup.rs` module + Tokio task that
  uses SQLite's `sqlite3_backup_*` C API for hot-snapshot
  every N hours to a configured dir. Off by default; opt-in
  via `CLIPTOWN_BACKUP_DIR` + `CLIPTOWN_BACKUP_INTERVAL_HOURS`.
- `scripts/restore-from-snapshot.sh`: stop the world
  container → swap the DB file → restart. Idempotent.
- One integration test: boot world, take a snapshot, mutate
  state, replace DB from snapshot, assert state rolled back.
- DEPLOY.md "Backups" section + retention/rotation advice.

**Size:** S–M.

**Closes:** "what happens if the docker volume corrupts"
worry. Lowest urgency in the phase — data loss is rare;
most of the visible-value work ships before this.

## Recommended sequencing

1. **First**: P5.A (Operator presence) — cheapest collab win;
   presence state powers B/C displays.
2. **Second**: P5.B (Per-operator audit visibility) — small
   follow-on to A using the same name-resolution lookup.
3. **Third**: P5.C (Soft-locks) — leans on A's avatar
   rendering and B's name display.
4. **Fourth**: P5.D (Observability) — independent of the
   collab arc, but lands before E so the deploy has eyes on
   it from minute one.
5. **Fifth**: P5.E (Deploy pipeline) — the same compose file
   orchestrates D's observability profile. Doing E before D
   would mean shipping a deploy story that gets immediately
   rewritten when D's compose profile lands.
6. **Sixth**: P5.F (Backup/restore drill) — lowest urgency;
   benefits from being testable against the live
   containerized world from E.

## What this doc is NOT

- **Not a binding plan.** Each theme gets its own brainstorm
  + plan when scheduled. The Phase 4 cadence (brainstorm →
  spec → plan → ship) holds.
- **Not a deadline / commitment.** Themes are sequenced for
  value and dependency, not time-boxed.
- **Federation (F2) stays deferred.** XL scope from Phase 4
  roadmap; small-team coordination on a single cliptown is
  plenty without cross-instance plumbing. Revisit if real
  demand surfaces.
- **Public SaaS / multi-tenant pivot is out.** Phase 5 frame
  is "2-5 operators on one shared cliptown," not "anyone can
  sign up." That's a different product question.
- **Corp SSO / external auth integration is out.** Bearer-
  token model is fine for a small team; SSO is a Phase 6+
  question if cliptown ever lands inside a company.
- **Agent-quality work** (smarter task decomposition, memory
  mgmt, recovery loops) is out. May resurface as Phase 6.

## Suggested next concrete action

Pick **P5.A** (Operator presence) for the first Phase 5 PR
cycle. Small scope, high felt value (the first time a
teammate's avatar appears in the sidebar is the moment
cliptown stops feeling like a solo tool), and lands the
operator-id↔name lookup primitive that B and C reuse.

**v1 scope decision (resolved 2026-05-17):** presence tracks
`focused_startup_id` only. Agent-possession overlap is a real
footgun but lives outside P5.A — possess is already a
single-slot operation (`OperatorPossess` replaces any prior
`__operator__` avatar in that startup), so the worst case is
last-write-wins, not silent data corruption. Add
possession-aware presence in a follow-up if real friction
shows up.
