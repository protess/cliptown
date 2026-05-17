# Changelog

## M16 — feat: auto-recovery on review failure (P6 Theme C) (2026-05-17)

Third and final Phase 6 PR. Closes the Phase 6 roadmap.
Before this, a task that the manager kept bouncing would sit
at `changes_requested` until `max_review_rounds` escalated
it to the operator. Now, opt-in per startup, the scheduler
tries to rescue the task by reassigning to an idle same-role
peer first.

- Migration 0015 adds
  `startups.auto_recovery_enabled INTEGER NOT NULL DEFAULT 0`
  + `startups.auto_recovery_max_attempts INTEGER NOT NULL
  DEFAULT 2`.
- `scheduler::tick` runs a new `auto_recovery_pass` after
  the auto-steal pass:
  - For each enabled startup, find `changes_requested` tasks
    where `review_round >= max_attempts`.
  - Pick an idle same-role peer (not the current assignee).
    Reuses the SQL pattern from P4 E3 `task_steal`.
  - UPDATE assignee + reset status to `queued` + reset
    review_round to 0 + bump updated_at. Same-row WHERE
    clause makes concurrent steals lose cleanly.
  - Audit row + `task_recovered` system_event with
    `strategy: "peer_reassign"`, severity `info`.
- New admin-only `StartupSetAutoRecovery {startup_id,
  enabled, max_attempts?}` ConsoleInbound. Mirrors P4 E3's
  `StartupSetAutoSteal`. `max_attempts == None` leaves the
  SQL default in place.
- Snapshot's `startups[*]` now carries
  `auto_recovery_enabled` + `auto_recovery_max_attempts`
  for client hydration.
- Frontend `StartupVM` + indexer parse both fields. New
  `AutoRecoveryToggle` pill in MainHeader (admin-only,
  teal color to distinguish from auto-steal's blue) with
  inline popover for enable + max_attempts input.
- `task_recovered` joins the toast-worthy SystemEvent set
  with a prettifier entry "T1 recovered e1 → e2
  (peer_reassign)".
- 4 new scheduler integration tests (reassigns when over
  threshold / skips when under / disabled is noop / skips
  when no idle peer). New http_smoke test pins the
  snapshot enrichment.

Phase 6 close: A self-review gates, B structured tool
surface, C auto-recovery. Agents now catch their own bad
artifacts before manager review, run real tests/lints with
audit, and the system rescues stuck tasks before they
escalate to a human.

## M16 — feat: structured tool surface for self-review (P6 Theme B) (2026-05-17)

Second Phase 6 PR. P6.A shipped the self-review gate but the
real verification (run tests, lint TS/Rust, diff against base)
needed shell access without losing auditability. This slice
adds the structured tool surface.

- New `crates/world/src/agent_tools.rs`:
  - `run_command(program, args, cwd, timeout)` — primitive
    that runs a shell command in a specific dir, captures the
    LAST 16KiB of stdout + stderr (UTF-8-safe truncation),
    enforces timeout (default 120s, max 600s), reports
    elapsed_ms + exit_code + timed_out flag.
  - `task_workdir(sid, tid)` returns the per-task workdir
    path `workspaces/<sid>/<tid>/workdir/` matching the
    layout in `packages/worker/src/execenv.ts`.
  - `sniff_test_command(workdir)` returns
    `(cargo, [test --quiet])` for Rust, `(pnpm, [test])`
    for Node, `(make, [test])` for Makefile workdirs, and a
    no-op fallback for unrecognized dirs.
- Three new MCP tools (29 → 32):
  - `run_tests {task_id, command?, timeout_secs?}` — runs
    the test suite in the per-task workdir. Optional
    `command` runs via `sh -c`; absent → uses
    `sniff_test_command`. Audit row captures exit_code +
    elapsed_ms.
  - `lint_artifact {task_id, artifact_path}` — dispatches
    by extension: `.ts`/`.tsx` → `npx tsc --noEmit`, `.rs`
    → `cargo check`, `.md`/`.json` → existing `verify`
    paths (in-process; no shell). Unsupported extensions
    → `unsupported_extension` error.
  - `read_artifact_diff {task_id, artifact_path,
    base_ref?}` — `git diff <base_ref> -- <artifact>`
    inside the per-task workdir. Defaults base_ref=HEAD.
- All three tools share the `require_task_workdir` gate:
  same-startup + assignee-only.
- 8 unit tests on the helper (stdout/stderr capture, exit
  code propagation, timeout, 16KiB cap, workdir layout,
  three sniff variants).
- `tests/backup.rs::restore_from_snapshot_rolls_back_state`
  was flaky under heavy parallel test load — now calls
  `pool.close().await` explicitly before the file swap so
  the WAL handle flushes deterministically.

P6.A's `markdown_lint` warn-stub still rides through
`verify`'s deferred path; P6.C will close the loop by
running these tools when self_review wants to upgrade
warn-checks to errors.

## M16 — feat: self-review gates (P6 Theme A) (2026-05-17)

First Phase 6 PR. The engineer agent now runs a pre-submit
check pipeline before `task_done` flips the task to
`awaiting_review`. Bad artifacts (missing files, empty
output, non-canonical paths) bounce back to the agent with a
structured `must_fix` list instead of riding into the
manager's review queue.

- Migration 0014 adds `tasks.self_reviewed_at INTEGER`,
  cleared on `task_request_changes` so the next re-submit
  triggers a fresh check.
- New `crates/world/src/self_review.rs` module:
  - `canonical_artifact_path(startup_id, task_id)` centralizes
    the canonical-path string used by `task_done`'s hard
    check + the soft gate.
  - `run(startup_id, task_id, artifact_path)` runs the v1
    pipeline: canonical_path → artifact_exists → json_lint
    (when applicable) → markdown_lint (warn-severity stub
    until P6.B's TS sidecar lint pipeline lands).
  - `record(pool, task_id, agent_id, outcome)` stamps
    `self_reviewed_at = unixepoch()` on pass + appends an
    audit_trail entry with the full outcome.
- New 29th MCP tool `self_review {task_id, artifact_path}`.
  Returns `{ok, must_fix:[{check, severity, message}]}`.
  Same-startup + assignee-only gates.
- `task_done` gains optional `auto_check: bool` (default
  true). When true, runs `self_review` mid-transition; if
  any check has `severity:"error"`, returns `Err(("must_fix",
  json))` instead of flipping status. `severity:"warn"` is
  informational only — doesn't block submission.
- `tools/list` count bumped 28 → 29.
- 6 new `self_review` integration tests pinning the gate
  semantics. `mcp_http::boot()` updated to pre-create the
  canonical artifact dir so the dispatch-routing test isn't
  dependent on leftover state.

Markdown / TS / Rust linting still ride the deferred stub.
P6.B wires the real linters; this slice ships the structural
pipeline so they have a place to plug in.

## M15 — feat: backup/restore drill (P5 Theme F) (2026-05-17)

Sixth and final Phase 5 PR. Closes the Phase 5 roadmap.
Adds the SQLite hot-snapshot daemon, restore script, and an
integration test that runs the full snapshot → mutate →
restore cycle on every CI build.

- New `crates/world/src/backup.rs`:
  - `snapshot_to(pool, path)`: writes a hot snapshot via
    `VACUUM INTO`. Path whitelist rejects quotes/backslashes
    (the splice is inline; binds aren't supported by VACUUM
    INTO).
  - `prune(dir, keep)`: deletes oldest snapshots beyond
    `keep`. Non-snapshot files in the dir are left alone.
  - `spawn_backup_tick(pool)`: opt-in periodic task gated on
    `CLIPTOWN_BACKUP_DIR`. Reads `CLIPTOWN_BACKUP_INTERVAL_
    HOURS` (default 6) and `CLIPTOWN_BACKUP_KEEP` (default
    14). Wired in `loop_::spawn_with_layout`. No-op when env
    is unset, so existing deploys see no behavior change.
- New `scripts/restore-from-snapshot.sh`: stop world → swap
  files → remove stale WAL/SHM → restart. Backs up the
  current live DB to `<live-db>.pre-restore` first so the
  operation is undoable. Refuses corrupt snapshots via a
  `sqlite3 ".tables"` precheck when the binary is available.
- 6 integration tests including the full restore drill:
  snapshot → mutate post-snapshot → swap files → re-open →
  assert pre-snapshot state survives, post-snapshot rolled
  back. Path-injection rejection + prune retention semantics
  pinned.
- DEPLOY.md gains "Backups" + "Restore drill" sections with
  the env-var reference, a sample restore against the docker
  compose volume path, and a pointer to the integration test.

Phase 5 close: A presence, B audit visibility, C soft-locks,
D obs artifacts, E docker deploy, F backup/restore.
Small-team coordination tool is now a real product surface.

## M15 — feat: docker deploy pipeline (P5 Theme E) (2026-05-17)

Fifth Phase 5 PR. DEPLOY.md described docker compose as an
option since Phase 3, but only the world image existed. Frontend
deployment was "build + serve yourself"; observability tooling
was "build it yourself." This slice closes both gaps + adds
tag-triggered GHCR release.

- New `packages/frontend/Dockerfile` (multi-stage: pnpm build →
  nginx serve) + `nginx.conf` porting the Vite dev-proxy rewrites
  (`/api/*`, `/ws/*`, `/metrics`) to nginx with SPA fallback.
- `docker-compose.yml` gains the `frontend` service (depends on
  `world` healthy, port 3000:80) + a profile-gated
  `observability` stack: `prometheus` (port 9090) +
  `grafana` (port 3001, anonymous-admin). Grafana auto-loads
  the P5 Theme D dashboard via provisioning; Prometheus loads
  the alert rules from the same docs/ tree.
- New `docs/observability/prometheus/prometheus.yml`, plus
  `docs/observability/grafana/provisioning/{datasources,
  dashboards}/*.yml` so Grafana hydrates fully on boot — no
  manual import, no datasource setup.
- `.github/workflows/release.yml`: tag-triggered (`v*`) matrix
  build of `cliptown-world` + `cliptown-frontend` images via
  `docker buildx`, multi-arch (amd64 + arm64), pushed to GHCR
  with both `:vX.Y.Z` and `:latest` tags. Cache via
  `type=gha,scope=<image>`.
- CI gains docker-build smoke steps for both images +
  `docker compose config` validation for both profiles.
- DEPLOY.md "Local: docker compose" section rewritten around
  the two-service default + the observability profile +
  "pull prebuilt from GHCR" option.

The compose `observability` profile completes the P5 Theme D
loop: dashboard JSON + alert YAML existed since slice D; now
the orchestration that brings them up lives next to them.

## M15 — feat: observability artifacts (P5 Theme D) (2026-05-17)

Fourth Phase 5 PR. The world has been exposing `/metrics` since
P3, but no Grafana dashboards or alert rules lived in the repo.
This slice ships both as importable files. P5.E will mount
them via the docker-compose `observability` profile.

- `docs/observability/grafana/cliptown-overview.json`:
  importable Grafana dashboard with 7 panels — tick rate (Hz),
  MCP call/error rates, task distribution by status, agent
  health buckets, per-startup budget % spent (with 80/95%
  thresholds), active startup count, cumulative MCP calls.
- `docs/observability/alerts/cliptown.yml`: Alertmanager rule
  YAML covering tick stall (<0.5Hz/30s), MCP error rate
  (>1/min/60s), budget warning (>95%/60s), agent lost
  (recently_lost present/30s warn), agent offline (offline
  present/60s alert).
- README adds an "Observability" section pointing at both
  files + noting the P5.E compose profile will pre-provision.
- New `observability_artifacts` test: dashboard JSON parses
  and every panel has targets + a title; alert YAML parses
  and every Prometheus expression references a known metric
  exported from `crates/world/src/metrics.rs`. Cheap, but
  catches typos before they land in ops surfaces.
- `serde_yaml` added as a dev-dep for the alert-YAML parser.

## M15 — feat: soft-locks on destructive actions (P5 Theme C) (2026-05-17)

Third Phase 5 PR. Two operators clicking "Force-Accept T1" at
the same time can no longer clobber each other. The second
click bounces with a friendly "locked by Alice — 25s" toast
while the first finishes.

- Migration 0013 adds `action_locks { id, lock_key UNIQUE,
  operator_id, acquired_at, expires_at }`. `lock_key` is a
  namespaced string (`task:T1:force_accept`,
  `operator:op_xyz:revoke`). `UNIQUE` is the test-and-set.
- New `crates/world/src/action_locks.rs`: `try_acquire`
  (returns `Err(Conflict(LockConflict))` on contention),
  `release`, `gc_expired` (returns dropped keys for
  broadcasting), `list_active`.
- `cmd_console`'s `OperatorForceAccept`, `OperatorForceFail`,
  and `OperatorRevoke` paths wrap with 30s soft-locks. Conflict
  reply: `{type:"error",reason:"locked_by",operator_name,
  expires_at}`. Lock release on success or via the GC tick.
- `loop_::spawn_with_layout` spawns a 5s lock GC tick that
  drops expired rows and emits `ActionUnlocked` so peers
  re-enable their UI without polling.
- New `ConsoleOutbound::ActionLocked { info }` +
  `ConsoleOutbound::ActionUnlocked { lock_key }` broadcasts
  on acquire/release/GC. Snapshot includes `action_locks: []`
  for hydration on new connections.
- Frontend `ActionLockVM` + `state.actionLocks`. Reducer
  handles `action_locked` / `action_unlocked` broadcasts and
  parses the snapshot field. `type:"error"` with
  `reason:"locked_by"` surfaces as a transient warn toast
  showing "Locked by Alice — 25s".
- 6 unit tests on the locks module (acquire/conflict/release/
  stale-sweep/gc/list_active). Test fixtures across
  `console_cmds`, `e2e_force_actions` now seed `operators`
  rows so the FK on `action_locks.operator_id` holds; broadcast
  assertions filter `ActionLocked`/`ActionUnlocked`
  infrastructure frames.

v1 deliberately doesn't disable buttons per-lock in Card/
Kanban — the server-side gate is the real safety, and the
toast on conflict covers the UX. Per-button disable can land
as a Theme G-style polish slice if friction shows.

## M15 — feat: per-operator audit visibility (P5 Theme B) (2026-05-17)

Second Phase 5 PR. Audit/history surfaces stop showing the
"operator" sentinel and start showing the actual operator who
took the action. Builds on P5.A's `PresenceAvatar` color hash.

- `ConsoleOutbound::Directive` carries new
  `author_display_name: Option<String>`. cmd_console's
  `OperatorDirective` handler resolves `identity.name` at
  emit time. Agent-sourced directives (peer review, manager
  feedback) pass `None` — frontend resolves from
  `state.avatars[author_id]` as before.
- Audit-trail JSON entries for `accept_proposal`,
  `reject_proposal`, `force_accept`, `force_fail` now include
  `operator_id`. Invisible until a future UI surfaces
  audit_trail; the data shape is right.
- Frontend `MessageVM.author_display_name`. ChatPanel
  renders operator-sourced messages as a colored 14px
  `PresenceAvatar` + `op:Alice` instead of the bare
  `operator` sentinel.
- console_emit test updated to destructure the new field.

`author_id` stays "operator" as a backwards-compatible
discriminator (existing clients match on it); the real
identity rides in `author_display_name`.

## M15 — feat: operator presence (P5 Theme A) (2026-05-17)

First Phase 5 PR. Two operators connected to the same cliptown
no longer work in the dark — they see each other in the TopBar
and as per-startup focus dots in the Sidebar. Foundation for
P5.B (operator-name audit) and P5.C (soft-locks "locked by …").

- New `crates/world/src/presence.rs` module: `PresenceRegistry`
  (Arc<RwLock<HashMap<operator_id, PresenceEntry>>>) +
  `upsert` / `drop_entry` / `gc` / `snapshot` helpers.
  `PRESENCE_TTL_SECS = 90` (3× the 30s heartbeat).
- `Handle` carries the registry. `loop_::spawn_with_layout`
  spawns a 30s GC tick that drops stale entries and broadcasts
  if any dropped.
- `ConsoleOutbound::OperatorPresence { v, presences }`
  re-broadcast on connect, disconnect, focus-change heartbeat,
  and GC-with-drops. `ConsoleInbound::PresenceHeartbeat { v,
  focused_startup_id? }` short-circuits in `handle_console`
  (doesn't traverse the world loop — presence isn't world
  state). The matching `cmd_console` arm returns a noop ack
  to preserve the exhaustive-match invariant.
- Frontend: `PresenceVM` + `state.presence` in the store,
  reducer case for `operator_presence`. Console emits a
  heartbeat on `selectedStartupId` change and every 30s.
- New `PresenceAvatar` component with deterministic 8-hue
  hash on `operator_id` so the same operator reads visually
  consistent across panels (a primitive Theme B will reuse).
- Sidebar renders up to 3 other-operator avatars per startup
  row (filtering self out). TopBar shows online operators
  next to the wordmark with a "+N" overflow pill.
- 6 unit tests on the registry helpers (upsert new/same/
  focus-change, drop, GC stale/at-boundary).

v1 scope (per the roadmap): focus is tracked at startup
granularity only. Possession-aware presence (which agent an
operator is possessing) deferred — last-write-wins on
`OperatorPossess` is acceptable for v1.

## M14 — feat: HistoryModal filtering + OperatorsPanel grouping (Theme G slice 5) (2026-05-17)

Fifth and final Theme G slice. Closes the last two items in the
roadmap's polish bucket from #75 ("HistoryModal could use richer
filtering. The operators panel could group by role.").

- `HistoryModal`: severity toggle chips (info/warn/alert/critical),
  kind substring filter, startup dropdown, "X of Y" count. Detail
  column now uses `prettifySystemEventPayload` (from slice 1) for
  readable summaries on E-theme events instead of JSON dumps.
- `OperatorsPanel`: rows grouped by role with section headers
  (admin → manager → viewer → other). Within each group rows
  sort by `created_at` ascending then name. Useful once operator
  count grows past a handful.

Pure frontend slices, both single-file. Type-check passes.

The Theme G bucket is now drained — all five slices closed in
the same week as Phase 4's E themes:
1. SystemEvent toasts + readable marquee (#80)
2. Admin toggles for peer-reviewer + auto-steal (#81)
3. Kanban blocked/deadline badges + steal flash (#82)
4. SkillsPanel revision history + revert (#83)
5. HistoryModal filtering + OperatorsPanel grouping (this PR)

## M14 — feat: SkillsPanel revision history + revert (Theme G slice 4) (2026-05-17)

Fourth Theme G slice. The skill_revert MCP tool (from #71) was
agent-callable but had no operator-console UI — the explicit
"UI deferred" carry-forward from #72.

- New ConsoleInbound variants: `SkillListRevisionsOperator`
  (read-only, any operator) and `SkillRevertOperator`
  (manager-or-above, mirrors SkillUpsertOperator's gate).
  Both use the existing `skills::list_revisions` /
  `skills::revert_to_revision` helpers, so the SQL gate stays
  in one place; `SkillRevertOperator` broadcasts
  `SkillChanged { kind: "revert" }` so any connected client's
  cache invalidates.
- Frontend `WorldState.skillRevisions` lazy-caches per-skill
  revision lists, keyed by skill_id (skill ids are globally
  unique). The reducer's `skill_changed` handler clears the
  cache for the affected skill on upsert/delete/revert so the
  next history-button click refetches.
- `SkillsPanel` adds a "⏱" history button per row. Clicking
  toggles a sub-panel listing revisions newest-first
  (rev_seq, timestamp, author, content length). Non-current
  rows get a "Revert" button gated by a confirm.
- 3 new console_cmds tests: viewer can list, manager can
  revert end-to-end, viewer is rejected on revert. Tests seed
  the `op_test` operator row so `append_revision`'s FK
  (`created_by_operator_id REFERENCES operators`) doesn't
  silently soft-fail — a latent gap in the existing
  skill_upsert_operator tests is now closed.

## M14 — feat: kanban blocked/deadline badges + steal flash (Theme G slice 3) (2026-05-17)

Third Theme G slice. E2 (#78) added `blocked_on` + `deadline_at`
to tasks; E3 (#79) added `task_stolen` system_events. The kanban
had no visual cue for either — operators only saw the marquee +
toasts from slice 1.

- `build_console_snapshot` (http.rs) extends the tasks SELECT
  with `blocked_on` + `deadline_at`. Frontend `TaskVM` gains
  both; `coerceTask` (extracted from the two indexer branches)
  parses them.
- `Card.tsx` renders two new badges in the meta row:
  `BlockedBadge` ("🔒 T_block6") when `blocked_on` is set,
  `DeadlineBadge` ("⏰ in 2h" / "⏰ overdue 60s" in red) when
  `deadline_at` is set. `flexWrap: "wrap"` lets the meta line
  break gracefully on narrow columns.
- `Kanban.tsx` watches `state.systemEvents` for `task_stolen`
  via a ts-watermark dedup, adds the task_id to a transient
  `highlightedTasks` set for 1.5s, and threads `highlighted`
  through the Card so the reassignment lands visually instead
  of silently re-rendering the assignee monogram. Card adds a
  blue ring + 240ms box-shadow transition.
- 1 new http_smoke test: snapshot tasks surface
  `blocked_on` + `deadline_at` (incl. null cases).

Closes the last two Phase 4 visual carry-forwards (E2 badges +
E3 reshuffle animation). All Phase 4 themes now have their
operator UI surface.

## M14 — feat: admin toggles for peer-reviewer + auto-steal (Theme G slice 2) (2026-05-17)

Second Theme G slice. E1 (#77) and E3 (#79) shipped the wire +
SQL for `agents.is_peer_reviewer` and `startups.auto_steal_*`,
but the admin couldn't flip either without raw SQL. This slice
adds both.

- `build_console_snapshot` (http.rs) enriches the snapshot:
  each `avatars[*]` carries `is_peer_reviewer` (joined from
  `agents` so we don't cascade-edit the 46 AvatarView literal
  sites), each `startups[*]` carries `auto_steal_enabled` +
  `auto_steal_after_secs`.
- Frontend `AvatarVM` / `StartupVM` gain the same fields;
  `coerceAvatar` / `coerceStartup` parse them from the snapshot.
- New `AgentsPanel` (admin-only, collapsible, mirrors
  OperatorsPanel) lists agents of the selected startup with a
  peer-reviewer checkbox. Wires `agent_set_peer_reviewer`.
- New `AutoStealToggle` (admin-only) in MainHeader — pill
  showing "Auto-steal: On (Ns) / Off" with an inline popover for
  flipping the flag + editing the threshold. Wires
  `startup_set_auto_steal`.
- 2 new `http_smoke` tests: snapshot startup rows surface
  auto-steal fields (incl. SQL defaults), snapshot avatar rows
  surface `is_peer_reviewer`.

Type-check passes. Backend `agent_set_peer_reviewer` /
`startup_set_auto_steal` ConsoleInbound handlers were already in
place from E1 + E3 — this slice is purely the surfacing.

## M14 — feat: SystemEvent toasts + readable marquee (Theme G slice 1) (2026-05-17)

First Theme G slice. The E2/E3 work landed three new SystemEvent
kinds (`task_unblocked`, `task_overdue`, `task_stolen`) but the
operator could only catch them by scrubbing the 1-line rotating
marquee or the History rail. Easy to miss while mid-action.

- `store.ts`: new `prettifySystemEventPayload(kind, payload)`
  exports a one-line summary for the three kinds
  ("T1 stolen by e2 ← e1 (auto)", "T1 unblocked from T0",
  "T1 overdue by 60s"). The reducer's `system_event` case now
  auto-pushes a `ToastVM` for these kinds in addition to the
  history append. Sticky for warn-or-above (overdue stays visible),
  transient for info-level (unblocked / stolen).
- `TopBar.tsx`: the marquee uses the same prettifier before
  falling back to `describeDetail`'s JSON.stringify, so the
  rotating banner reads "T1 unblocked from T0" instead of
  `{"task_id":"T1","blocker_id":"T0"}`.

Pure frontend slice — no protocol or world changes. Type-check
passes; ship-gate e2e references `task_escalated` (untouched).

Admin toggles for `is_peer_reviewer` and `auto_steal_enabled`
deferred to Theme G slice 2 — they need protocol-level shape
extensions (AvatarView + StartupVM new fields) so worth their
own focused PR.

## M14 — feat: async work-stealing (Theme E3) (2026-05-16)

Fourth Phase 4 PR. When one engineer's queue stays warm and a peer
sits idle, the scheduler should rebalance. Two surfaces:

- **Manual**: 28th MCP tool `task_steal {task_id}`. Caller must be
  idle, same-startup, share the assignee's role, NOT be the current
  assignee, and the task must be `queued`. Same-row WHERE clause
  guards the UPDATE so a concurrent steal loses the race cleanly
  (`lost_race`). Emits `task_stolen` system_event with `mode=manual`.
- **Auto**: opt-in per-startup. Migration 0012 adds
  `startups.auto_steal_enabled INTEGER NOT NULL DEFAULT 0` and
  `startups.auto_steal_after_secs INTEGER NOT NULL DEFAULT 60`.
  Scheduler post-dispatch pass walks every enabled startup, picks
  queued tasks whose `updated_at` is older than the threshold AND
  whose assignee isn't idle, reassigns to an idle same-role peer.
  Skips when the current assignee is already idle (no churn).
  Emits `task_stolen` with `mode=auto`.
- Admin-only `StartupSetAutoSteal {startup_id, enabled, after_secs?}`
  ConsoleInbound — flip the flag without touching SQL.
- Audit trail captures both modes with `actor=stealer`.
- 8 integration tests: happy path + not_idle / cross_startup / role
  mismatch / self_steal / not_stealable / auto reassigns when stale +
  busy / auto disabled when flag off / auto skips when assignee idle.

Operator-side UI button + kanban swim-lane reshuffle on `task_stolen`
deferred to Theme G (console). The wire/data path lands here so G
has something to render.

## M14 — feat: blocking dependencies + deadlines (Theme E2) (2026-05-16)

Third Phase 4 PR. Task graph captures `parent_id` but had no "this
task can't start until X finishes" relation and no deadlines. Real
coordination needs both.

- Migration 0011 adds `tasks.blocked_on TEXT REFERENCES tasks(id)
  ON DELETE SET NULL`, `tasks.deadline_at INTEGER`, and
  `tasks.deadline_notified_at INTEGER` (dedup stamp). Two partial
  indexes for the scheduler's scans.
- **Scheduler blocking gate**: a queued task with `blocked_on` set
  is held until the dependency reaches a terminal state. On the
  unblocking tick: clear the column, emit
  `task_unblocked` system_event, fall through to dispatch.
- **Scheduler deadline scan** (post-dispatch): non-terminal tasks
  with `deadline_at < now AND (notified_at IS NULL OR <
  deadline_at)` get one `task_overdue` system_event each + the
  stamp updates. Editing `deadline_at` clears the stamp so a new
  boundary fires fresh.
- New 27th MCP tool `task_set_blocking {task_id, blocked_on?,
  deadline_at?}`. Manager-or-assignee (same gate as
  `task_set_preference`). Null clears; self-blocking rejected.
- `POST /api/admin/tasks` accepts the new fields on creation.
- `scheduler::tick` signature gains `&event_tx`; legacy
  out_bus-only tests pass the new arg.
- 6 integration tests: blocked-non-terminal-held / unblock-clears-
  and-emits / overdue-emits-once / dedup-on-second-tick /
  no-deadline-quiet / terminal-bypassed / editing-deadline-
  re-emits.

Auto-directive to the blocker's manager on overdue deferred to
Theme E follow-up — system_event surface is enough for the
console kanban swimlane (Theme G).

## M14 — feat: peer review beyond manager review (Theme E1) (2026-05-16)

Second Phase 4 PR (per #75). `task_request_changes` was
manager-only; real teams want a designer reviewing engineer output
or an engineer reviewing a founder spec.

- Migration 0010 adds `agents.is_peer_reviewer INTEGER NOT NULL
  DEFAULT 0` + a partial index on `is_peer_reviewer = 1`.
- `mcp_dispatch::handle_task_request_changes` permission check
  becomes a disjunction: manager-of-task OR (peer_reviewer AND
  same-startup AND not the assignee). Self-review is still
  refused. The audit_trail entry carries `actor = "manager" |
  "peer"` so the org graph stays readable.
- New admin-only ConsoleInbound `agent_set_peer_reviewer
  {agent_id, is_peer_reviewer}`. Peer-reviewer status is a
  cross-startup privilege grant — manager privilege isn't enough
  to flip the flag.
- 4 mcp-handler tests: peer-flagged can review / unflagged peer
  rejected / peer-as-assignee can't self-review / manager still
  writes `actor:"manager"`. 3 cmd_console tests: admin flips /
  viewer forbidden / unknown agent_id returns not_found. TS
  bindings regenerated.

Operator-side UI (mark/unmark peer reviewer button on agent rows)
deferred — Theme G bucket.

## M14 — feat: local-LLM smoke (Theme F1) (2026-05-16)

First Phase 4 PR. Validates the local-first deploy narrative from
#55: cliptown should run end-to-end against a self-hosted ollama
in ~30s on a developer laptop.

- **`scripts/smoke-ollama.sh`** — wraps `smoke-real-llm.sh` with
  the ollama-shaped env preset (`OPENAI_BASE_URL` = local ollama,
  `OPENAI_API_KEY=ollama`, backend-specific `*_MODEL`). Pre-flight
  checks: ollama serving + the requested model pulled (no surprise
  multi-GB downloads). codex default; `BACKEND=opencode` to test
  the other adapter path. claude-code rejected with a clear
  message — needs a translator proxy.
- **`packages/adapters/opencode/test/model_spec.test.ts`** — pins
  the `provider/model` parsing contract documented in DEPLOY.md.
  5 cases: bare model, ollama prefix, model with colon (e.g.
  `qwen2.5:7b`), other providers, empty-modelID edge case.
  `splitProviderModel` is now `export`ed for the test.
- **`docs/DEPLOY.md`** Local LLM section gains a "Verify with
  scripts/smoke-ollama.sh" subsection with both backends + the
  zero-cost callout.

CI does NOT run the smoke (requires GPU + a pulled model). It's a
developer-laptop smoke. The opencode model-spec test runs in CI
and catches a refactor that would silently break the local route.

## M13 — feat: hash operator tokens at rest (2026-05-15)

Closes the "Token hashing deferred" note from #61.

- Migration 0009 recreates `operators` (SQLite has no DROP NOT NULL)
  with nullable `token` + new `token_hash TEXT`.
- `auth::hash_operator_token` = SHA-256 hex.
- `validate_operator_token` looks up by hash first, falls back to
  plaintext for pre-0009 rows; successful plaintext match rewrites
  in place (lazy migration).
- `operator_create` stores only the hash; plaintext returned once.

## M13 — feat: world-side periodic execenv GC daemon (2026-05-15)

Closes the "World-side periodic auto-GC deferred" note from the
script PR.

- New `execenv_gc` module with `run_pass` + `spawn`. Selection
  mirrors `scripts/gc-execenv.sh`: terminal-state tasks past age
  cutoff. Artifacts dir not touched.
- Opt-in via `CLIPTOWN_EXECENV_GC_ENABLED=1`. Overrides:
  `_AGE_DAYS` (7), `_INTERVAL_HOURS` (6), `CLIPTOWN_WORKSPACES_ROOT`.
- 4 unit tests; DEPLOY.md updated.

## M13 — feat: skill_revert (rollback to historical revision) (2026-05-15)

Closes the "Rollback deferred" note from #67. Schema was ready;
this PR ships the mutation path.

- `skills::revert_to_revision` loads historical row, sets it live,
  appends a NEW revision pointing at the same content. History
  stays linear.
- 26th MCP tool `skill_revert {skill_id, rev_seq}`. Same-startup
  gate.
- Emits `SkillChanged { kind: "revert" }`.
- 4 new DAO tests.

Operator-side ConsoleInbound + frontend UI deferred — MCP path is
callable today.

## M13 — feat: operator identity on hello reply + admin-only UI gate (2026-05-15)

Closes a known limit on #69. OperatorsPanel + SkillsPanel global
toggle were admin-only on the server but always visible client-side.

- New ConsoleOutbound `HelloOk { operator_id, operator_name, role }`.
  Emitted after token validation; token not echoed.
- Frontend reducer populates `state.currentOperator`.
- `OperatorsPanel` returns null when role ≠ admin (hooks run first
  so React's hook-order invariant holds; also hides pre-hello to
  avoid flash-in).

## M13 — feat: is_global toggle + indicator in SkillsPanel (2026-05-15)

Finishes the global-skills surface. #68 added the backend flag +
`skill_set_global` ConsoleInbound; this PR adds the UI knob.

- `SkillWithAttachments` carries `is_global`. `SkillVM` mirrors it.
- Per-row globe toggle (admin-only on server). 🌐 badge appears
  next to the name when set.

## M13 — docs: Phase 4 roadmap brainstorm (2026-05-15)

Closes the "Phase 4 brainstorm needed" note from the Phase 3
roadmap. Catalogues candidate themes — peer review, time-bounded
dependencies, work-stealing (the deferred Theme E), local-LLM
polish, operator UX polish, and a sketched multi-cliptown
federation theme — with sizing + recommended sequencing.

Recommended first PR cycle: Theme F1 (local-LLM smoke) to validate
the local-first narrative from #55 before stacking new
coordination features.

Each theme will get its own brainstorm spec when picked up; this
doc is intentionally a strategic-direction sketch, not a binding
plan.

## M13 — feat: operator management panel in the console (2026-05-15)

Final Phase 3 carry-forward — Theme B frontend surface.
`operator_list` / `_create` / `_revoke` / `_set_role` ConsoleInbound
variants have been there since #61; this PR adds the operator UI
so admins no longer need raw WS to use them.

- New `OperatorsPanel.tsx`, mounted below SkillsPanel. Collapsed
  by default. On expand, sends `operator_list` to hydrate.
- Per-row: name, role select (viewer / manager / admin), Revoke
  button (confirm-prompted).
- Footer: name input + role select + Create button. On success
  the server returns the minted `opt_*` token inline — the panel
  shows it in a `MintedTokenBanner` with a copy-now warning that's
  dismissed by the operator after copying. Cliptown never displays
  the token again.
- Store reducer handles the four `{type:"ok", kind:"operator_*"}`
  reply envelopes — populates `state.operators` on list, appends
  on create + stashes `mintedOperatorToken`, filters on revoke,
  updates role in-place on set_role.
- Non-admin callers see an empty list (server returns `forbidden`,
  silently caught client-side). No role-detect on the hello reply
  yet; the panel just gracefully handles empty.

Closes the M13 roadmap — every Phase 3 theme + every carry-forward
follow-up has landed.

## M13 — feat: globally-visible skills (2026-05-15)

Roadmap carry-forward. Skills were strictly startup-scoped; style
guides / debug primers had to be duplicated. Adds an admin-only
`is_global` flag that auto-surfaces a skill in every agent's
execenv regardless of `agent_skills` attachment.

- Migration 0008 adds `is_global INTEGER NOT NULL DEFAULT 0` +
  partial index. `skills::for_agent` UNIONs attached rows with
  `is_global = 1`; DISTINCT-by-id prevents double-listing.
- `skills::set_global` DAO. New admin-only ConsoleInbound
  `skill_set_global {skill_id, is_global}`.
- SkillChanged broadcasts emit `set_global` / `clear_global` kinds.
- 4 new DAO tests.

Agents cannot flag their own skills global by design. Frontend UI
deferred (single boolean wire).

## M13 — feat: skills file attachments (2026-05-15)

Roadmap carry-forward. Skills could only carry a single `content_md`
blob; bundles often need supporting files (templates, JSON configs,
examples). Adds a `skill_files` sibling table + 2 MCP tools +
worker-side materialization into the execenv.

- Migration 0006 adds `skill_files (id, skill_id FK, name, content,
  created_at, updated_at)` with `UNIQUE (skill_id, name)` and
  `ON DELETE CASCADE` from `skills`.
- `skills` crate gains `upsert_file` / `delete_file` / `list_files`
  / `file_name_is_valid`. File names: alphanumeric + `- _ .`; `..`,
  slashes, empty strings rejected. Content reuses the existing 32
  KiB cap. Cross-startup ownership enforced.
- `AttachedSkill` gains `files`; `/api/agents/:id/skills` returns a
  `files` array per skill. Worker's `prepareWorkdir` writes each at
  `<workdir>/skills/<skill-name>/<file-name>` alongside the main
  `.md`. Names validated at upload → path traversal impossible.
- 2 new MCP tools: `skill_file_upsert` + `skill_file_delete`.
  Tools/list grows 22 → 24. Mutations emit `SkillChanged` events
  with new kinds `file_upsert` / `file_delete`.
- 8 new DAO tests: upsert roundtrip, in-place update, cross-startup
  reject, delete + missing, cascade-from-skill-delete, bad/good
  names, for_agent includes-files.

Operator-console UI deferred — the MCP path is agent-callable; a
dedicated operator file editor can come when there's pressure.

## M13 — feat: skills revision history (2026-05-15)

Roadmap carry-forward — final skills item. `skills.content_md` was
overwritten in place on every upsert; no audit, no rollback target.

- Migration 0007 adds `skill_revisions (id, skill_id FK, rev_seq,
  content_md, created_at, created_by_agent_id?, created_by_operator_id?)`
  with `UNIQUE (skill_id, rev_seq)` and FK cascade. Index on
  `(skill_id, rev_seq DESC)` for newest-first reads.
- New `skills::Author { Agent(&str) | Operator(&str) | Unknown }`
  enum + `upsert_with_author` that records who wrote each revision.
  The legacy `upsert()` stays for unit tests; it routes to
  `upsert_with_author(.., Unknown)`. Production call sites updated:
  `mcp_dispatch::handle_skill_upsert` passes `Author::Agent`,
  `cmd_console::SkillUpsertOperator` passes `Author::Operator`.
- Revision append is best-effort after the live update succeeds —
  losing history is preferable to losing user-authored content;
  failure logs `tracing::warn!`.
- `skills::list_revisions(pool, startup_id, skill_id)` returns the
  full revision history, ownership-gated (cross-startup peek → error).
- New 23rd MCP tool: `skill_list_revisions {skill_id, limit?}`.
- 7 new tests: first-upsert rev_seq=1, increment, author agent,
  author operator, cross-startup reject, not-found, FK cascade on
  skill delete.

Rollback (revert-to-revision) deferred — schema supports it but
needs a UX surface before shipping.

## M13 — feat: skills content authoring in the operator console (2026-05-15)

Roadmap carry-forward. SkillsPanel only supported attach/detach;
operators had to use MCP tools or SQL for content authoring.

- 2 new manager-gated ConsoleInbound variants:
  `skill_upsert_operator` + `skill_delete_operator`. Routes through
  the same `skills::upsert`/`skills::delete` paths as the MCP tools.
  `skill_id` on upsert is wire-compat-only — server resolves by
  `(startup_id, name)`.
- `SkillsPanel.tsx` gets `+ New skill` button + per-row ✎ edit /
  ✕ delete with confirm. Inline editor for both create + edit.
- Editor starts blank for edit too — the WS snapshot ships skill
  metadata only (`len`/`updated_at`); re-fetching `content_md` per
  skill would inflate every snapshot. Operators paste/re-type;
  upsert resolves by `(startup_id, name)` so the existing row
  updates in place.
- 4 new integration tests cover upsert (create + update-in-place),
  delete, and viewer-forbidden on both.

## M13 — feat: cost variance telemetry (2026-05-15)

Final Theme C deferred bit. Tasks can carry a `cost_estimate_usd`
hint; when actual spend lands via `report_budget`, the world emits a
`task_cost_variance` system_event when |actual−estimate|/estimate ≥
50%. Operators get an early warning when a routing decision (model
choice, prompt complexity) blew the estimate.

- Migration 0005 adds nullable `cost_estimate_usd REAL` to `tasks`.
  NULL = no estimate, variance comparison skipped.
- `POST /api/admin/tasks` accepts the field. Validated as finite +
  non-negative at the boundary; bad inputs return 400 with
  `bad_cost_estimate`.
- `cmd_worker::ReportBudget` joins on the task row after a
  successful budget apply; when both estimate and cost are present
  and crossed the ±50% threshold, emits the system_event. Overrun
  ⇒ `severity = "warn"`; underrun ⇒ `severity = "info"`. Within
  threshold = silent. Multi-spawn / resumed runs may emit twice for
  the same task — the operator console dedupes by `task_id` (no
  cliptown-side dedup state).
- 4 new integration tests cover overrun + underrun + within +
  no-estimate paths.

## M13 — feat: smoke against remote world targets (2026-05-15)

Phase 3 Theme A carry-forward. The smoke harness was local-only —
remote operators (Fly.io / staging) had no scripted way to verify a
deploy was healthy.

- **`POST /api/admin/tasks`** (new): operator-token + manager-role
  gated task creation. Mirrors the SQL `INSERT INTO tasks` the
  smoke previously did directly. Validates startup + assignee (same
  startup) before insert. Returns `{id, status, startup_id}`. 7
  integration tests cover auth, role gate, queued/proposed paths,
  unknown-startup, cross-startup.
- **`scripts/smoke-real-llm.sh`** gains `WORLD_REMOTE_URL=https://...`
  mode: skips cargo build + world boot, derives http+ws bases from
  the URL, posts to the new admin endpoint instead of raw SQL. The
  worker spawns locally and talks to the remote `/ws/worker`. FS +
  SQL checks (artifact-on-disk, execenv layout, skill files, budget
  row) are skipped in remote mode — no client-side access to either.
  A clean adapter exit + responsive `/health` is the success signal.
- `DEPLOY.md` "Smoke against a deployed instance" section rewritten
  with both modes documented.

Local mode (no env var) is unchanged.

## M13 — chore: structured tracing events across hot paths (2026-05-15)

Phase 3 Theme D follow-up. `/metrics` (#51) covered the metrics
endpoint but the spec for Theme D also called out "structured
`tracing` spans through hot paths (MCP dispatch, scheduler tick,
view broadcast)." That side stayed deferred.

This PR adds **event-pair tracing** (enter + exit-with-elapsed) on
the three dispatch hot paths:

- `mcp_dispatch::dispatch` — per-call enter/exit with tool +
  agent_id + startup_id + corr_id + outcome (ok|error) + elapsed_us.
- `scheduler::tick` — emits `tick_complete` debug event when
  dispatched>0 or elapsed>5ms, with tick_seq + dispatched +
  elapsed_us. Quiet ticks skip the log so a default-level
  subscriber stays clean.
- `cmd_console::dispatch` — enter/exit with command_kind (16
  variants enumerated) + operator_id + operator_role + outcome.

We use the event-pair pattern rather than `Span::entered()` because
the WS loop's task awaits SQL inside these handlers — a `!Send`
Span guard would break `tokio::spawn`'s Send bound. Structured
backends correlate enter+exit via the `corr_id` field; console
subscribers see two compact events per dispatch.

Default subscriber filter (`RUST_LOG=info`) sees only error-path
events; debug-level filter (`RUST_LOG=cliptown_world=debug`)
surfaces the full enter/exit pair for latency replay.

## M13 — feat: admin-only operator management commands (2026-05-15)

Phase 3 Theme B follow-up. #52 landed the `operators` table + role-
aware token validation but stopped at "schema only; surface comes
when multi-operator deploys arrive." This PR adds the surface:
admins can now provision additional operators without touching SQL.

Four new `ConsoleInbound` variants, all gated `at_least(Admin)`:

- **`operator_list`** — `{operators: [{id, name, role, created_at}]}`.
  Read-only. The cheapest gate to verify before any mutation.
- **`operator_create`** — `{name, role}` → mints a fresh
  `opt_<uuid>` token + returns it inline. Token is generated server-
  side (not provided by the admin) so it lands in the response body
  exactly once; the admin copies it. Duplicate names → `name_taken`,
  unknown role → `bad_role`.
- **`operator_revoke`** — `{operator_id}` → DELETE on row. Self-
  revoke refused (`cannot_revoke_self`) — would lock the calling
  admin mid-session.
- **`operator_set_role`** — `{operator_id, role}` → UPDATE on row.
  Self-demotion to non-admin refused (`cannot_demote_self`).

Tokens are plain UUID v4s prefixed `opt_`. No hashing today —
deferred until the deployment story actually has more than one
operator (with proper rotation tooling).

8 new integration tests in `console_cmds.rs` cover list/create/
revoke/set_role × admin-allowed + viewer-rejected + edge cases
(bad role, dup name, self-revoke, self-demote). TS bindings
auto-regenerated.

Theme B is now functionally closed: schema (#52) + surface (this
PR). Frontend UI for operator management remains a separate task
(operator console doesn't yet ship a settings panel).

## M13 — chore: claude-code adapter honors CLAUDE_CODE_MODEL (2026-05-15)

Closes the last Theme C known-limit from #58 / #59. claude-code
adapter now reads `CLAUDE_CODE_MODEL` from `opts.env` and forwards
it to the CLI as `--model <id>`. Combined with worker's
`modelEnvForBackend("claude_code") = "CLAUDE_CODE_MODEL"`, a per-
task `preferred_model` now reaches the claude CLI on equal footing
with codex (`CODEX_MODEL_ID`) and opencode (`OPENCODE_MODEL`).

- `packages/adapters/claude-code/src/index.ts` — append `--model
  <id>` to the CLI args when `env.CLAUDE_CODE_MODEL` is set. Gated
  on `useJsonOutput` so the fixture-cli used by contract tests
  (which doesn't speak `--model`) never sees the flag.
- `packages/worker/src/main.ts::modelEnvForBackend` returns
  `"CLAUDE_CODE_MODEL"` for `claude_code` instead of null. Worker
  spawn now plumbs `--preferred-model` into the env var for all
  three adapters.
- Test updated to assert the new mapping (`main_args.test.ts`).

Theme C wire is fully closed across all three adapters.

## M13 — feat: per-task worker spawn (Theme C Option B) (2026-05-13)

Phase 3 Theme C follow-up #2 — the supervisor side. Completes the
chain Theme C started: `tasks.preferred_*` columns → SQL JOIN in
scheduler → SpawnConfig → `worker --real --task-id --prompt
--preferred-backend --preferred-model` → adapter env override
honoring (#58).

Opt-in via **`CLIPTOWN_PER_TASK_WORKERS=1`**. With the env var set:

- `api_startups::create_startup` skips the legacy long-running
  daemon spawn — agents have SQL rows but no live worker until a
  task dispatches.
- `scheduler::tick` joins `agents` + `startups` to assemble a
  `SpawnConfig { task: Some(TaskSpawn { prompt, preferred_* }) }`
  and calls `supervisor.spawn_agent`. A canonical prompt is built
  from the task title/description with the artifact path baked in.
- `agent_supervisor::spawn_child` appends `--real --task-id
  --prompt --preferred-backend --preferred-model` when
  `cfg.task.is_some()`. Existing watch_loop already returns on
  clean exit (no respawn) so each task is a one-shot.
- The out_bus liveness check inverts polarity in per-task mode: an
  out_bus entry means a previous worker is still mid-task for this
  agent — skip, don't double-spawn.
- Rollback on `spawn_agent` failure (worker bin missing, etc.)
  mirrors the existing out_bus failure path: SQL flips back to
  queued, avatar.status back to idle.

Env var unset = legacy daemon path unchanged. Smoke harness keeps
working (it sets `CLIPTOWN_TEST_DISABLE_SUPERVISOR=1` and spawns
its own worker out-of-band).

3 new supervisor tests cover the env-var toggle, per-task argv
shape, and the legacy-shape negative case. 1 new scheduler test
confirms the env-var-off fallback.

Worker-side adapter spawn already honors `--preferred-*` from #58
so no worker changes were needed.

## M13 — chore: worker honors per-task routing preferences (2026-05-13)

Phase 3 Theme C follow-up #1. `task_assigned` carries
`preferred_backend` + `preferred_model` since #53, but nothing
downstream read them. This PR wires the consumer end.

- **`packages/worker/src/main.ts`** — two new CLI args:
  `--preferred-backend` (overrides `--backend` for adapter
  selection in `--real` mode) and `--preferred-model` (passed to
  the resolved adapter via its model env var).
- **`modelEnvForBackend(backend)`** helper — maps `codex →
  CODEX_MODEL_ID` and `opencode → OPENCODE_MODEL`. claude_code
  returns null today: the adapter doesn't thread a model knob
  (CLI has `--model`, the adapter wrapper doesn't expose it) —
  flagged as a known limitation.
- Real-mode spawn picks adapter via the resolved (preferred-or-
  default) backend, logs the override decision, and merges the
  model env var into `SpawnOpts.env` so the adapter inherits it.
- 5 new tests in `packages/worker/test/main_args.test.ts` cover
  the CLI parse + the env-mapping helper.

**Not in this PR:** the agent supervisor doesn't yet read
`tasks.preferred_*` from SQL when spawning workers — it's still
per-agent-default. The next wiring step (smaller follow-up) is to
extend `SpawnConfig` + the supervisor's spawn path so the world
auto-injects `--preferred-*` based on the task being dispatched.

## M13 — chore: execenv GC script (2026-05-13)

Carry-forward housekeeping #2 from the Phase 3 roadmap. Per-task
execenv workdirs at `workspaces/<sid>/<tid>/workdir` accumulate
forever — operators had no recipe to reap them short of `rm -rf`.

- `scripts/gc-execenv.sh` — bash + sqlite3. Selects tasks in
  terminal states (`done` / `failed` / `escalated`) AND
  `updated_at` older than `--days N` (default 7), removes their
  workdir. Artifacts at `workspaces/<sid>/artifacts/` are
  preserved so audit replays keep working. `--dry-run` flag for
  preview; `--db` + `--workspaces` overrides for the docker /
  Fly.io path layouts.
- Read-only SQL access; safe to run while the world is up.
- `docs/OPERATOR.md` gains a "Garbage-collect old execenv workdirs"
  recipe in the common-ops section.

World-side periodic auto-GC deferred — explicit operator-run is
safer; we can promote to a scheduler task later if it gets tedious.

## M13 — chore: bench gate flipped to hard fail + recalibrated for CI (2026-05-13)

Carry-forward housekeeping #1 from the Phase 3 roadmap. The bench
workflow was running `continue-on-error: true` since M10.1 because
the dev-box baselines (22.966 µs / 970,556 msgs/s on Apple Silicon)
were ~3x off the ubuntu-latest CI numbers, so flipping the gate
would have flagged every PR.

- Captured 3 successful CI medians from recent post-Phase-3 runs
  (`tick_latency_real_loop`: 72.283 / 70.933 / 71.550 µs;
  `console_dispatch_throughput`: 360,983 / 364,830 / 360,632 msgs/s).
- `bench/baselines.json` bumped to v3 with CI-grade values
  (baseline=72 µs tick, 361,000 msgs/s throughput). Original CI
  samples preserved in `_ci_samples_*` fields so the next re-baseline
  has a reference point.
- `.github/workflows/bench.yml`: dropped `continue-on-error: true` on
  the regression-check step. Bench failures now fail the PR.
- `crates/world/benches/world_bench.rs`: fix compile error introduced
  by Phase 3 Theme B (#52) — `Cmd::HandleConsoleMsg` now requires an
  `identity` field. Bench passes `OperatorIdentity::admin_for_tests()`
  to match the new contract. This is why every bench job since #52
  reported "build failed" — the gate was already broken, just silent.

## M13 — docs: local-first deploy + local LLM routing (2026-05-13)

Docs-only follow-up to Theme A. The original deploy guide led with
Fly.io, which sends the wrong signal — cliptown's most interesting
LLM workflows are local (ollama, vLLM, LM Studio), and a cloud VM
can't reach your local GPU.

- **`docs/DEPLOY.md`** restructured: native local → docker compose →
  **local LLM via ollama (new)** → Fly.io → other targets. The local
  LLM section documents how the codex / opencode adapters pick up
  `OPENAI_BASE_URL` + provider-specific model env vars to route at
  `http://localhost:11434/v1` (ollama's OpenAI-compatible endpoint),
  including the `host.docker.internal` quirk for docker compose
  callers. The claude-code adapter needs a translator proxy for
  local backends — flagged but out of scope.
- **`README.md`** Deploy section reordered to lead with `pnpm dev` /
  `docker compose up -d`; Fly.io framed as the "share with
  collaborators" mode.
- Added Vercel to the "doesn't fit" list in DEPLOY.md alongside Cloud
  Run — both are serverless-leaning, neither supports cliptown's
  stateful single-process model.

No code changes. Adapter env-var pass-through (`...process.env` spread)
was already in place since M1, so the local LLM routing path "just
works" once you set the right vars; this PR is purely making that
discoverable.

## M13 — Phase 3 Theme C: per-task routing preferences (2026-05-13)

Fifth Phase 3 theme. Tasks were routed implicitly to whatever
backend/model was provisioned on the agent at startup; cheaper
models couldn't be opted into for trivial subtasks.

- **`crates/world/migrations/0004_task_routing_preferences.sql`**
  (new) — adds nullable `preferred_backend` + `preferred_model` to
  `tasks`. NULL falls back to the agent's provisioned default.
- **`WorkerOutbound::TaskAssigned`** (ts-rs auto-export) carries the
  two new optional fields; the scheduler reads them from the queued-
  task row and forwards them on dispatch.
- **`task_set_preference` MCP tool** (22 tools total now) — manager-
  or-assignee can set/clear either field. Both managers (budget POV)
  and assignees (load POV) are reasonable callers. Cross-startup
  blocked, stranger callers refused. Audit row appended +
  `task_routing_changed` system_event broadcast for operator audit.
- Worker-side adapter honoring is documented but not enforced — the
  field arrives on `task_assigned`; worker implementations decide
  whether to override the spawn arguments. Carry-forward note.

Cost variance telemetry (estimated-vs-actual emission to
system_events) deferred — needs estimate inputs first.

## M13 — Phase 3 Theme B: operator RBAC (2026-05-13)

Fourth Phase 3 theme. Console access is no longer a single shared
`CLIPTOWN_OPERATOR_TOKEN`. Each operator has a row in the new
`operators` table with one of three roles, and mutating console
commands check the role before touching SQL or the broadcast bus.

- **`crates/world/migrations/0003_operators.sql`** (new) — `operators`
  table `(id, name, token UNIQUE, role CHECK(viewer|manager|admin),
  created_at)`. Seeded with `op_default / dev-token / admin` so the
  existing dev workflow keeps working.
- **`crates/world/src/auth.rs`** — `validate_operator_token` returns
  a typed `OperatorIdentity { id, name, role }` instead of `()`. Table
  lookup first, with `CLIPTOWN_OPERATOR_TOKEN` env-var fallback that
  returns a synthetic admin so legacy deployments keep working. 4 new
  unit tests cover seeded admin / unknown / viewer + manager rows /
  role ordering.
- **`crates/world/src/loop_.rs`** — `Cmd::HandleConsoleMsg` carries
  the `OperatorIdentity` captured at WS-hello validation so the loop
  can hand it to `cmd_console::dispatch` per inbound frame.
- **`crates/world/src/cmd_console.rs`** — added per-arm role gate.
  Viewer ≥ : `Hello`, `OperatorPossess`, `OperatorUnpossess`,
  `OperatorMove`, `OperatorRecheckBackends` (read-ish, no SQL writes
  beyond avatar state). Manager ≥ : `OperatorDirective`,
  `OperatorAcceptProposal`, `OperatorRejectProposal`,
  `OperatorForceAccept`, `OperatorForceFail`, `SkillAttach`,
  `SkillDetach`. Forbidden returns `{"type":"error","reason":"forbidden"}`
  before any SQL or broadcast — symmetric with the cross-startup
  rejection pattern.
- **`crates/world/src/http.rs::handle_console`** captures the identity
  from `validate_operator_token` and forwards it on each inbound
  command.
- **Tests** — `console_cmds.rs` gains 3 RBAC integration tests
  (viewer rejected on force_accept, viewer can possess+move, manager
  can force_accept). All 5 existing `tests/*.rs` files updated to
  pass an admin identity via `OperatorIdentity::admin_for_tests()`.

Admin-only operator-management commands (provision/revoke operator
rows, role changes) deferred — schema is ready, surface comes when
multi-operator deploys actually arrive.

## M13 — Phase 3 Theme D: observability (2026-05-13)

Third Phase 3 theme. cliptown now exposes a Prometheus-style
`/metrics` endpoint so ops can scrape liveness and load.

- **`/metrics`** HTTP endpoint emits text exposition format. Metrics:
  - `cliptown_mcp_calls_total` (counter, all MCP tool call attempts)
  - `cliptown_mcp_errors_total` (counter, calls that returned mcp_error)
  - `cliptown_agents{health="..."}` (gauge, 4 labels for P2.1 health buckets)
  - `cliptown_startups_active` (gauge)
  - `cliptown_budget_spent_usd{startup_id="..."}` + `cliptown_budget_cap_usd{...}` (gauges)
  - `cliptown_tasks{status="..."}` (gauge, 8 status labels)
  - `cliptown_tick_seq` (counter, monotonic loop liveness)
- **`crates/world/src/metrics.rs`** (new) — hand-rolled text format
  renderer + atomic global counters. No external prometheus crate
  dependency. 2 unit tests cover zero-state rendering + counter
  increment.
- **`mcp_dispatch::dispatch`** increments `mcp_calls_total` per call
  and `mcp_errors_total` per error.

Scrape latency: O(active startups + tasks). Single-digit ms on
current scale; revisit caching if it climbs past 100ms.

Structured tracing spans through hot paths deferred — incremental
add as needed.

## M13 — Phase 3 Theme F: documentation pass (2026-05-13)

Second Phase 3 theme. cliptown's contributor and operator docs were
sparse. README focused on Phase 0 details that have since rotted; no
ARCHITECTURE, no OPERATOR, no AGENT guide.

- **`README.md`** rewritten — current status reflects sealed Phase
  0-2, Phase 3 underway. Test counts updated. New "Real-LLM smoke"
  section. "Where things live" index points at the new docs.
- **`ARCHITECTURE.md`** (new) — topology diagram, per-component
  walkthrough (world, worker, adapters, frontend, protocol), MCP
  tools catalog summary (21 tools across task lifecycle / knowledge
  / world interaction / skills), architectural invariants, spec
  cross-references.
- **`docs/OPERATOR.md`** (new) — operator console guide. Health
  buckets, possess flow, kanban actions (accept/reject proposal,
  force accept/fail, review rounds), skills management, directives,
  budget controls, system events, SQL spelunking recipes.
- **`docs/AGENT.md`** (new) — what cliptown looks like from inside
  an adapter-spawned CLI. Workdir layout, CLAUDE.md contract, all
  21 MCP tools categorized, hook events, budget ladder, sandboxing
  rules, common patterns (finish task / propose subtask / read peer
  artifact / author skill).

No code paths affected; all tests stay green.

## M13 — Phase 3 Theme A: production deploy story (2026-05-13)

First Phase 3 theme. cliptown now ships a Dockerfile, docker-compose
config, and a Fly.io app config + deploy guide so operators can run it
for real workloads.

- **`Dockerfile`** (new) — multi-stage build. Stage 1 builds the rust
  world binary (release); stage 2 installs the worker's pnpm deps
  (workers run via tsx, no transpile step); stage 3 is a debian-slim
  runtime carrying the world binary + node 20 + worker source +
  migrations + `cliptown.toml`. Runs as the unprivileged `cliptown`
  user. Healthcheck against `/health`.
- **`docker-compose.yml`** (new) — single-service local-prod equivalent
  with persistent volumes for `/data` (SQLite) and `/workspaces`
  (per-task execenv). Forwards provider keys from a gitignored `.env`.
- **`fly.toml`** (new) — single-VM single-region Fly.io app config.
  Mounts a persistent volume to `/data`. Health check + auto-start.
  Cliptown is single-process today; scale up VM size, not replicas.
- **`docs/DEPLOY.md`** (new) — quickstart for docker-compose, full
  Fly.io walkthrough (launch / volume / secrets / deploy / rotate /
  rollback), sketches for AWS / GCP / K8s / bare VPS, secrets pattern
  doc (`CLIPTOWN_OPERATOR_TOKEN`, `CLIPTOWN_AGENT_SECRET_*`, provider
  keys).
- **`README.md`** — adds a Deploy section pointing at DEPLOY.md.

Verified: `docker build` succeeds, `docker run` boots the world
server, `/health` returns `{"ok": true}` on first request.

### Known limitations carried forward

- `scripts/smoke-real-llm.sh` boots its own world; parameterizing it
  to target a remote deploy is a follow-up. DEPLOY.md documents the
  manual verification path in the meantime.

## M12 — P2.2 skills system (Phase 2 MVP, 2026-05-13)

Per-startup reusable markdown skills attached many-to-many to agents.
At `--real` adapter spawn the worker fetches the agent's attached
skills and writes each as `<workdir>/skills/<name>.md` (alongside
CLAUDE.md and the workspaces symlink from P2.3). CLAUDE.md gains an
"Available skills" section listing each skill's name and relative
path.

- **Schema:** `skills` (workspace-scoped, `UNIQUE(startup_id, name)`)
  + `agent_skills` (M:N attachment). Migration `0002_skills.sql`.
- **DAO:** `crates/world/src/skills.rs` with 9 inline unit tests
  (upsert/list/attach/detach/delete/for_agent + bad-name +
  oversize-content + cross-startup). Names constrained to
  `[A-Za-z0-9_-]{1,64}` (filesystem-safe); content capped at 64 KB.
- **MCP tools (5 new, catalog 16 → 21):** `skill_upsert`,
  `skill_list`, `skill_attach`, `skill_detach`, `skill_delete`.
  All enforce cross-startup checks.
- **HTTP API:** `GET /api/agents/:agent_id/skills` returns
  `{skills: [{name, content_md}]}` for the worker. Bearer auth via
  `<agent_id>:<secret>` reuses `crate::auth::validate_agent_secret`
  (env-var-backed; default `dev-secret`). 403 on path-vs-bearer
  agent mismatch; 401 on bad secret.
- **Worker:** new `skills_client.ts::fetchSkillsForAgent` +
  `prepareWorkdir` extension write skills into the execenv. Warn-
  and-continue on fetch failure (an agent with no skills proceeds
  with an empty list).
- **Smoke:** seeds `smoke-skill-deploy` + attaches it (§5.5);
  post-spawn asserts the file + CLAUDE.md reference (§7.6).

### Known limitations carried forward

- Skill content authoring UI (create/edit/delete) still routes through
  MCP `skill_upsert` / `skill_delete`. SkillsPanel covers read + attach
  + detach. Inline content editor is a future PR.
- No global (non-workspace) skills.
- No file attachments beyond the markdown content_md body.
- No versioning / history (upsert is mutable; latest wins).

## M12 — skills broadcasts + read+attach/detach UI (2026-05-13)

P2.2 follow-up that closes two of five known-limitations from the
initial Phase 2 MVP entry.

- **`ConsoleOutbound::SkillChanged`** broadcast on every skill
  mutation (upsert / delete / attach / detach). All 5 MCP skill_*
  handlers + the 2 new console-side arms emit. `kind="upsert"`
  carries the full listing row so the frontend reducer applies in
  place without a follow-up fetch.
- **`ConsoleOutbound::SkillsSnapshot`** delivered right after
  `WorldViewSnapshot` at console connect. Payload is `{sid: [{id,
  name, len, updated_at, attachments}]}` for every startup with
  skills.
- **`ConsoleInbound::SkillAttach` / `SkillDetach`**: operator-side
  attach/detach commands (no agent caller required). Skill authoring
  (create/edit/delete) still flows through MCP `skill_upsert` /
  `skill_delete` for now — heavier editor UX deferred.
- **`SkillsPanel.tsx`** in the operator console: lists skills for
  the currently-possessed startup, attached-agent chips (click to
  detach), "Attach to…" dropdown of unattached agents.

Known limitations retired by this section: "no frontend skill
management UI" partially (read + attach/detach is in; create/edit/
delete still via MCP) and "no `skill_changed` broadcasts" fully.

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

## M12 — P2.1 daemon health buckets (2026-05-12)

Replaces cliptown's binary worker-liveness signal (WS connected vs
closed) with a 4-state Health enum so the operator console doesn't
confuse a 5-minute network blip with a hard crash.

- **`crates/world/src/health.rs`** (new pure module) — `Health` enum
  + `derive(now_ts, last_seen, connected, is_operator) -> Health`.
  Thresholds: `RecentlyLost` ≤ 5 min, `Offline` ≤ 6 d,
  `AboutToGc` ≤ 7 d (last 24 h before GC), beyond 7 d back to
  `Offline`. Operator avatars and clock skew both forced Online.
- **`AvatarView`** carries `last_seen_at: Option<i64>` (updated on
  `RegisterWorker` / `HandleWorkerMsg`, preserved through
  `UnregisterWorker`) and `health: Health` (refreshed every
  `Cmd::Tick` before the view broadcast). `RegisterWorker` /
  `UnregisterWorker` also derive health + broadcast immediately so
  the operator console reflects state changes without waiting for
  the next tick.
- **Frontend `AvatarVM`** mirrors the shape; `PixiStage.tsx` sets
  `sprite.alpha` from `ALPHA_BY_HEALTH` (`online: 1.0`,
  `recently_lost: 0.7`, `offline: 0.4`, `about_to_gc: 0.3`).
- **Tests:** 8 inline unit tests for `health::derive` + 3 integration
  tests booting `loop_::spawn` (register sets last_seen + Online;
  HandleWorkerMsg refreshes; Unregister preserves + RecentlyLost).

## M11 — real bench harness (2026-05-12)

Replaces the two placeholder benches in `crates/world/benches/world_bench.rs`
with measurements that drive a real `loop_::spawn` world handle:

- **`tick_latency_real_loop`** times one `Cmd::Tick` round-trip
  (`move_sys::step_all` + `scheduler::tick` +
  `proximity::compute_and_emit` + `view_tx.send`). The watch receiver
  is cloned per iter so `.changed()` actually waits for the next tick.
- **`console_dispatch_throughput_100_msgs`** fires 100
  `Cmd::HandleConsoleMsg` with oneshot replies. The dispatcher's
  `serde_json::from_value` parse-error early return gives a fast
  reply without DB writes or broadcast — measures the
  mpsc → parse → oneshot round-trip.

`bench/check.mjs` swaps the `1000_div_median_us` extract recipe for
`100_div_median_us`. `bench/baselines.json` carries fresh
dev-box-captured numbers and renames the throughput key to
`world.console_dispatch_throughput_msgs_per_sec`. CI gate stays
`continue-on-error: true` until more ubuntu-latest samples land —
that flip is a separate follow-up.

## M11 — hook bridge parity (2026-05-12)

`codex` and `opencode` adapters now actually flow hook events. Both
previously advertised `[session_stop, session_error]` but never POSTed
anything to the HTTP hook bridge they stood up — dead code.

- **codex**: in-adapter streaming JSONL parser
  (`packages/adapters/codex/src/event_parser.ts`) converts
  `item.started` / `item.completed` for `command_execution` (tool name
  normalized to `"shell"`) and `mcp_tool_call` (tool name from
  `item.tool`) into pre_tool/post_tool HookEvents. Process exit emits
  session_stop on 0 or session_error with stderr_tail on non-zero.

- **opencode**: rebuilt around `opencode serve --port 0 --pure` +
  REST/SSE. The adapter spawns a headless opencode server, subscribes to
  `/event` SSE, and maps `message.part.updated` frames to HookEvents
  via `event_mapper.ts` — with proper per-callID dedup so pre_tool fires
  once (first `running` transition) and post_tool fires once (first
  terminal status). `session.idle` is the terminal signal. `opencode run
  --format json` is abandoned because it only emits already-completed
  tool frames.

- **HTTP hook bridge** is removed from codex/opencode (was dead). Still
  used by claude-code where the CLI's `settings.json` hook script
  contract genuinely needs it.

- **claude-code**: a smoke-driven fix — claude CLI 2.1.x ignores the
  `CLAUDE_CODE_SETTINGS` env var; the adapter now also passes
  `--settings <path>` so PreToolUse/PostToolUse/Stop hooks fire. The
  shared bridge also learned to read `tool_name` from claude payloads
  (it only checked `tool` before, so HookEvent.tool came out empty).

- Capability advertising now matches reality:
  `claude_code`/`codex`/`opencode` all carry
  `[pre_tool, post_tool, session_stop, session_error]`.

§ 11.9 smoke verified across all three backends — worker.log shows
named tools (`Write`, `ToolSearch`, `mcp__cliptown__task_done` for
claude; `shell`, `task_done` for codex; `apply_patch`,
`cliptown_task_done` for opencode) and a closing `session_stop`.

## Phase 0 — bring-up complete (2026-05-11)

Phase 0 closes with all 9 spec invariants proven at the Rust layer and the
real-LLM ship gate (§ 11.9) self-implemented end-to-end against a real
`claude` CLI. Phase 1 begins with the Phase 2 backlog from
`docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md`.

### What ships in Phase 0

- **World server** (`crates/world/`): SQLite-backed mpsc-routed loop with
  16 MCP tools, axum HTTP + WS surface, room A\* pathfinding, task
  scheduler, budget ladder (warn/95/pause), agent supervisor, sandbox
  path resolver.
- **Worker** (`packages/worker/`): WS client, MCP correlation, mock and
  real-LLM run modes, supervisor-respawn semantics.
- **Adapters** (`packages/adapters/{claude-code,codex,opencode}/`): unified
  `BackendAdapter` contract with hook bridge, MCP-at-the-world HTTP wiring,
  contract-tested cross-adapter.
- **Frontend** (`packages/frontend/`): React + Pixi 2D console, kanban,
  chat panel, possess transition, system event feed, 14 Playwright tests.

### Test counts at seal

| Layer | Tests |
|---|---|
| `cargo test -p cliptown-world` | 213 |
| `pnpm -F @cliptown/worker test` | 62 |
| `pnpm -F @cliptown/adapter-{core,claude-code,codex,opencode} test` | 12 |
| `pnpm -F @cliptown/frontend test` (Playwright) | 14 |

### M9.10 — real-LLM E2E (§ 11.9)

The longest milestone of Phase 0. Closed in 9 PRs across two arcs:

**Architecture (one merge train, #19 → #25):**
- `A1'` MCP-over-HTTP at the world (route + 16-tool catalog + bearer auth)
- `A2` worker `--real` flag spawns the adapter against world MCP
- `A3` `scripts/smoke-real-llm.sh` runs the chain locally with colored output
- `B` `pnpm -F @cliptown/e2e-real-llm start` runs it with structured JSON
- `C` was shipped then reverted in #25 — cliptown is open source and we
  don't want fork contributors to need an Anthropic API key in CI secrets

**Hardening (#26 → #29):**
- `#26` First real run surfaced three smoke-execution bugs (pnpm
  workspace discovery, script arg re-shelling, world's per-agent random
  secret unreachable from out-of-band workers). All fixed; smoke passes
  green against `claude` 2.1.138 OAuth, haiku-shaped output at canonical
  `workspaces/<sid>/artifacts/<tid>.md` path, task state machine
  transitions to `awaiting_review`.
- `#27` Ship-gate § 11.9 cell lifts from "real-LLM only — M9.10"
  placeholder to a runner pointer + verified-pass timestamp.
- `#28` `CLIPTOWN_TEST_DISABLE_SUPERVISOR=1` silences nine
  `spawn_agent failed` warnings per smoke run.
- `#29` Budget telemetry: `claude --output-format json` lets the adapter
  scrape `total_cost_usd`; worker forwards as `report_budget` WS frame;
  world's `apply_report` uses CLI cost when present, falls back to the
  pricing table otherwise. Smoke output now shows
  `budget_spent_usd ≈ $0.31` instead of `$0` — the cap (default $0.50)
  has real teeth.
- `#31` codex + opencode adapters lifted to A2-equivalent. `codex exec
  --json` + `-c mcp_servers.cliptown.{url,bearer_token_env_var}` +
  `--dangerously-bypass-approvals-and-sandbox` (full-auto skips shell
  but not MCP). `opencode run --format json --pure --dir <cwd>
  --dangerously-skip-permissions` with per-spawn `opencode.json` in the
  workspace. `pickAdapter` drops the `not_yet_supported_in_real_mode`
  throws. Bundled with a `fix(m9.10)` commit for the MCP HTTP
  notifications response: claude tolerated `{}` 200; rmcp 0.6+ used by
  codex rejected it. Spec-compliant 202 + empty body now.

### Phase 0 hardening (TODOS.md → Completed)

- **P3 emit_system_event JSON fallback**: malformed payloads now broadcast
  the raw string instead of silently degrading to `Value::Null`, so the
  audit log and operator console stay in sync. Loud-fail with
  `tracing::error!`.

### M10.1 — performance regression gate

- `cargo bench -p cliptown-world` runs two criterion benches (placeholder
  tick latency + mpsc throughput) and writes JSON estimates to
  `target/criterion/`.
- `bench/check.mjs` reads criterion's medians, converts to baseline units,
  and fails (exit 1) if any metric regresses by more than `tolerance_pct`
  (20% default).
- `.github/workflows/bench.yml` runs the gate on every PR. Phase 0 ships
  the gate as `continue-on-error: true` while the CI baselines stabilize
  vs developer hardware; flip to a hard gate after a few CI samples.
- `pnpm -F @cliptown/frontend bench:fcp` (added 2026-05-11) runs the
  Playwright FCP bench against a production `vite build`. Ceilings: 300ms
  for `/console`, 500ms for `/town/:id`. Asserts directly in the test, so
  failure surfaces as a Playwright test failure (no separate comparator
  needed for the frontend metrics). Local measurements at seal: /console
  ≈ 150ms, /town/:id ≈ 320ms — both well under their ceilings.

### Known limitations carried into Phase 1

- Adapter budget tracking + hook flow: closed under M11 (PR #36).
- Frontend FCP bench: closed under M10.1 follow-up (PR #35) —
  `pnpm -F @cliptown/frontend bench:fcp` runs against a production
  `vite preview` build, no longer `.skip`'d.
- Criterion benches: closed under M11 real bench harness (this section).
- `bench.yml` CI gate runs as `continue-on-error: true` while baselines
  stabilize vs developer hardware. Flip to a hard gate after a handful
  of CI samples.

### Phase 2 backlog (from M9.10 spec)

Three patterns imported from multica that didn't block § 11.9:

- **P2.1 — Daemon health buckets**: 4-state worker liveness
  (online/recently_lost/offline/about_to_gc) so transient network blips
  don't surface to the operator console as crashes. ~2-3h.
- **P2.2 — Skills system**: reusable per-task capabilities stored as
  markdown + files, attached to agents, written into the per-task
  workdir at spawn. ~1-2 weeks. Depends on P2.3.
- **P2.3 — Per-task execenv directories**: `{workspaces}/{sid}/{tid}/`
  with workdir + injected CLAUDE.md + skill files + 7-day GC. ~3-5 days.
  Prerequisite for P2.2.
