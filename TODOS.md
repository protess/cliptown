# TODOS

## Open

_(empty)_

## Completed

### M15 feat — docker deploy pipeline (P5 Theme E) — 2026-05-17
**Source:** Fifth Phase 5 PR (per #85 roadmap). PR `<TBD>`.

Was: only the world image had a Dockerfile; frontend deploy was "build it yourself"; observability tooling was "build it yourself"; no tag-triggered release path existed.

Fixed: new `packages/frontend/Dockerfile` (pnpm build → nginx serve with proxied `/api/*` `/ws/*` `/metrics`). `docker-compose.yml` adds `frontend` service + profile-gated `observability` stack (prometheus + grafana with the P5.D dashboard + alert rules auto-provisioned). New `.github/workflows/release.yml` tag-triggered (`v*`) matrix build pushes multi-arch images to GHCR. CI gains docker-build smoke for both images + compose validation for both profiles. DEPLOY.md docker section rewritten around two-service default + observability profile + GHCR pull option.

### M15 feat — observability artifacts (P5 Theme D) — 2026-05-17
**Source:** Fourth Phase 5 PR (per #85 roadmap). PR `<TBD>`.

Was: `/metrics` Prometheus endpoint existed since P3 but no Grafana dashboards or Alertmanager rules lived in the repo. Operators had to build their own.

Fixed: `docs/observability/grafana/cliptown-overview.json` ships an importable dashboard with 7 panels (tick rate, MCP calls/errors, tasks by status, agents by health, budget %, active startups, cumulative MCP calls). `docs/observability/alerts/cliptown.yml` ships 5 alert rules (tick stall, MCP error rate, budget warning, agent lost/offline). README "Observability" section points at both. New `observability_artifacts` test parses both files and verifies every alert expression references a known metric — catches typos before they land in ops. `serde_yaml` added as dev-dep.

### M15 feat — soft-locks on destructive actions (P5 Theme C) — 2026-05-17
**Source:** Third Phase 5 PR (per #85 roadmap). PR `<TBD>`.

Was: with two operators on the same cliptown, "Force-Accept T1" clicked simultaneously would clobber each other silently — no server-side gate, no UI feedback that another operator was mid-action.

Fixed: migration 0013 adds `action_locks` table with `UNIQUE(lock_key)` as test-and-set. New `crates/world/src/action_locks.rs` (acquire/release/gc/list_active). `cmd_console::OperatorForceAccept` / `OperatorForceFail` / `OperatorRevoke` wrap with 30s locks; conflict → `locked_by` error. New `ActionLocked` / `ActionUnlocked` ConsoleOutbound broadcasts; snapshot carries `action_locks`. 5s lock GC tick in `loop_::spawn_with_layout` drops expired rows + broadcasts unlocks. Frontend `ActionLockVM` + reducer; `type:"error", reason:"locked_by"` surfaces as a transient warn toast "Locked by Alice — 25s". 6 unit tests + test fixture updates across console_cmds/e2e_force_actions for the new operators FK + lock broadcast filter. v1 doesn't disable buttons per-lock — server-side gate + toast is the v1 UX.

### M15 feat — per-operator audit visibility (P5 Theme B) — 2026-05-17
**Source:** Second Phase 5 PR (per #85 roadmap). PR `<TBD>`.

Was: audit + history surfaces showed the opaque `operator` sentinel for operator-sourced actions. With 2-5 operators on one cliptown, "Who did this?" is the most common question and the UI had no answer.

Fixed: `ConsoleOutbound::Directive` carries `author_display_name: Option<String>`; cmd_console resolves `identity.name` at emit time. Audit-trail JSON for `accept_proposal` / `reject_proposal` / `force_accept` / `force_fail` includes `operator_id`. Frontend `MessageVM.author_display_name` parsed; ChatPanel renders operator-sourced messages with a 14px `PresenceAvatar` + `op:Alice` instead of `operator`. `author_id` stays "operator" sentinel for backward-compat discriminator. console_emit test updated.

### M15 feat — operator presence (P5 Theme A) — 2026-05-17
**Source:** First Phase 5 PR (per #85 roadmap). PR `<TBD>`.

Was: two operators connected to the same cliptown couldn't see each other. No way to know if a teammate was looking at the same startup, mid-edit, or already inside the kanban you're about to touch.

Fixed: new `crates/world/src/presence.rs` registry on `Handle` with TTL'd entries (90s, 3× heartbeat). `ConsoleOutbound::OperatorPresence` broadcasts on connect/disconnect/focus-change/GC. `ConsoleInbound::PresenceHeartbeat` short-circuits in `handle_console`. Frontend `Console.tsx` emits 30s heartbeats keyed off `selectedStartupId`. New `PresenceAvatar` with deterministic 8-hue hash on `operator_id` (reusable for B's audit visibility). Sidebar shows up to 3 other-operator avatars per startup row; TopBar shows online operators with "+N" overflow. 6 unit tests on the registry helpers. v1 = startup-level focus only; possession-aware presence deferred.

### M14 feat — HistoryModal filtering + OperatorsPanel grouping (Theme G slice 5) — 2026-05-17
**Source:** Fifth and final Theme G slice. Drains the last two roadmap items (#75) — "HistoryModal richer filtering" + "OperatorsPanel grouped by role". PR `<TBD>`.

Was: HistoryModal showed every event with no filter and JSON-stringified payloads; OperatorsPanel was a flat alphabetical list with no role grouping.

Fixed: HistoryModal adds severity toggle chips (info/warn/alert/critical), kind substring filter, startup dropdown, "X of Y" count; detail column uses `prettifySystemEventPayload` for E-theme events. OperatorsPanel buckets rows by role (admin → manager → viewer → other) with section headers + counts, sorted by `created_at` then name within each group. Pure frontend; type-check passes.

Theme G bucket fully drained — all five slices (toasts, admin toggles, kanban badges, skill history, this) closed in one week.

### M14 feat — SkillsPanel revision history + revert (Theme G slice 4) — 2026-05-17
**Source:** Fourth Theme G slice. Closes the #72 "skill_revert UI deferred" carry. PR `<TBD>`.

Was: the `skill_revert` MCP tool existed (since #71) but operators had no UI for it. To roll back, an admin had to either possess as an agent or hand-craft a WS frame.

Fixed: new `SkillListRevisionsOperator` (read-only, any operator) and `SkillRevertOperator` (manager-or-above) ConsoleInbound variants reusing the existing `skills::list_revisions` / `skills::revert_to_revision` helpers. Revert broadcasts `SkillChanged { kind: "revert" }`. Frontend `skillRevisions` cache lazy-loads per skill; reducer invalidates on upsert/delete/revert so the next history-button click refetches. SkillsPanel gains a "⏱" history button per row → sub-panel listing revisions newest-first with author + size + Revert button (gated by confirm). 3 new console_cmds tests cover happy path + viewer-forbidden. Tests seed the `op_test` operator row so `append_revision`'s FK doesn't soft-fail silently — closes a latent gap.

### M14 feat — kanban blocked/deadline badges + steal flash (Theme G slice 3) — 2026-05-17
**Source:** Third Theme G slice. Closes the E2 (#78) "kanban blocked/deadline badges" + E3 (#79) "kanban reshuffle animation on task_stolen" carry-forwards. PR `<TBD>`.

Was: E2 stored `blocked_on` + `deadline_at` on tasks and E3 fired `task_stolen` system_events, but the kanban gave no visual signal for either — operators only saw the marquee + toasts.

Fixed: snapshot extends tasks SELECT with `blocked_on` + `deadline_at`. Frontend `TaskVM` parses both; Card renders `BlockedBadge` + `DeadlineBadge` in the meta row (overdue → red). Kanban watches systemEvents for `task_stolen` via a ts-watermark dedup, holds the task_id in a transient highlight set 1.5s, threads `highlighted` to Card which paints a blue ring with a 240ms box-shadow transition. 1 new http_smoke test covers the snapshot enrichment.

### M14 feat — admin toggles for peer-reviewer + auto-steal (Theme G slice 2) — 2026-05-17
**Source:** Second Theme G slice. Closes the E1 (#77) + E3 (#79) "operator-side admin UI deferred" notes. PR `<TBD>`.

Was: E1 added `agents.is_peer_reviewer` and E3 added `startups.auto_steal_enabled` + `auto_steal_after_secs` to SQL, but the only way to flip them was raw SQL or hand-crafted WS frames.

Fixed: `build_console_snapshot` enriches the snapshot — avatars carry `is_peer_reviewer` (joined from `agents` so the 46 AvatarView literal sites don't need cascading edits), startups carry both auto-steal fields. Frontend `AvatarVM` / `StartupVM` parse them. New admin-only `AgentsPanel` (collapsible, lists agents of the selected startup with peer-reviewer checkbox) and `AutoStealToggle` pill in `MainHeader` (inline popover for enable + threshold). Both wire to ConsoleInbound handlers that already existed from E1 + E3. 2 new http_smoke tests cover the snapshot enrichment incl. SQL defaults.

### M14 feat — SystemEvent toasts + readable marquee (Theme G slice 1) — 2026-05-17
**Source:** First Theme G slice. Closes "operator-side surface for E2/E3 events" deferred from #78 + #79. PR `<TBD>`.

Was: `task_unblocked` / `task_overdue` / `task_stolen` SystemEvents only showed in the rotating 1-line marquee + History rail, with JSON.stringify'd payloads ("{"task_id":"T1","blocker_id":"T0"}"). Easy to miss while mid-action and unreadable when caught.

Fixed: new `prettifySystemEventPayload` exported from `store.ts` renders one-line summaries. The `system_event` reducer auto-pushes a `ToastVM` for the three kinds (sticky for warn+, transient for info). TopBar marquee uses the same prettifier before falling back to JSON.stringify. Pure frontend slice — type-check passes; no protocol changes.

### M14 feat — async work-stealing (Theme E3) — 2026-05-16
**Source:** Fourth Phase 4 PR (per #75 roadmap). PR `<TBD>`.

Was: queued tasks lived with their original assignee even when that engineer was buried and a same-role peer sat idle. No way to rebalance short of operator-driven `OperatorForceFail`+re-route.

Fixed: migration 0012 adds `startups.auto_steal_enabled` (default 0) + `auto_steal_after_secs` (default 60). New MCP tool `task_steal {task_id}` — caller must be idle, same-startup, same-role as the current assignee, not-self; task must be `queued`. Same-row WHERE clause guards UPDATE (race → `lost_race`). Scheduler post-dispatch auto-steal pass reassigns stale queued tasks when the flag is on AND assignee is busy AND an idle same-role peer exists; skips when assignee is already idle (no churn). Admin-only `StartupSetAutoSteal` ConsoleInbound for the flip. `task_stolen` system_event surfaces both modes. 8 integration tests. Operator-side UI button + kanban swim-lane shuffle deferred to Theme G.

### M14 feat — blocking dependencies + deadlines (Theme E2) — 2026-05-16
**Source:** Third Phase 4 PR (per #75 roadmap). PR `<TBD>`.

Was: task graph had `parent_id` but no "this can't start until X finishes" relation, and no deadlines.

Fixed: migration 0011 adds `tasks.blocked_on`, `deadline_at`, `deadline_notified_at` (dedup). Scheduler blocking gate holds queued tasks until dependency hits terminal state, then emits `task_unblocked`. Post-dispatch deadline scan emits one `task_overdue` per task per deadline-boundary (dedup via the stamp; editing the deadline clears the stamp so a new boundary fires fresh). New MCP tool `task_set_blocking {task_id, blocked_on?, deadline_at?}` (manager-or-assignee, self-blocking rejected). `POST /api/admin/tasks` accepts both fields. `scheduler::tick` signature gains `&event_tx`. 6 integration tests. Auto-directive to blocker's manager deferred to Theme E follow-up.

### M14 feat — peer review beyond manager review (Theme E1) — 2026-05-16
**Source:** Second Phase 4 PR (per #75 roadmap). PR `<TBD>`.

Was: `task_request_changes` was structurally manager-only — caller had to be parent-task's assignee or recorded `agents.manager_id`. Real teams want cross-role review (designer reviews engineer, etc.) but the schema didn't support it.

Fixed: migration 0010 adds `agents.is_peer_reviewer INTEGER NOT NULL DEFAULT 0`. `task_request_changes` permission becomes manager OR (peer_reviewer AND same-startup AND not-self). Audit `actor` discriminates `"manager" | "peer"`. New admin-only `AgentSetPeerReviewer` ConsoleInbound — admin-only because peer-reviewer is a cross-startup privilege grant. 4 mcp-handler tests + 3 console tests. Operator-side UI deferred to Theme G.

### M14 feat — local-LLM smoke (Theme F1) — 2026-05-16
**Source:** First Phase 4 PR. Validates the local-first narrative from #55. PR `<TBD>`.

Was: DEPLOY.md (#55) documented `OPENCODE_MODEL=ollama/...` and the codex+ollama path, but nothing actually smoke-tested it. Operators following the guide had no confidence the end-to-end loop worked.

Fixed: `scripts/smoke-ollama.sh` wraps `smoke-real-llm.sh` with the ollama env preset (OPENAI_BASE_URL + provider-specific `*_MODEL`). Pre-flight checks both ollama serving + model pulled. codex default; `BACKEND=opencode` for the other path. claude-code rejected (needs translator proxy). 5 new unit tests on `splitProviderModel` pin the `provider/model` parsing contract — runs in CI even though the full smoke is laptop-only. DEPLOY.md gains a Verify subsection.

### M13 feat — hash operator tokens at rest — 2026-05-15
**Source:** Closes the "Token hashing deferred" note from #61. PR `<TBD>`.

Was: `operators.token` stored plaintext bearers — SQLite snapshot leak exposes every active token.

Fixed: migration 0009 recreates the table with nullable `token` + new `token_hash TEXT`. `auth::hash_operator_token` (SHA-256 hex; 128-bit UUID entropy doesn't need a slow KDF). `validate_operator_token` hashed lookup first, plaintext fallback for pre-0009 rows — successful plaintext match rewrites the row in place (lazy migration). `operator_create` stores only the hash. Seeded `op_default` keeps working via fallback.

### M13 feat — world-side periodic execenv GC daemon — 2026-05-15
**Source:** Closes the "World-side periodic auto-GC deferred" note. PR `<TBD>`.

Was: `scripts/gc-execenv.sh` covered operator-driven cleanups; unattended deploys had no scheduled sweep.

Fixed: new `execenv_gc` module (`run_pass` + `spawn`). Selection mirrors the script: terminal-state tasks past age cutoff. Opt-in via `CLIPTOWN_EXECENV_GC_ENABLED=1` with `_AGE_DAYS`, `_INTERVAL_HOURS`, `CLIPTOWN_WORKSPACES_ROOT` overrides. 4 unit tests; DEPLOY.md secrets section updated.

### M13 feat — skill_revert (rollback to historical revision) — 2026-05-15
**Source:** Closes the "Rollback deferred" note from #67. PR `<TBD>`.

Was: #67 shipped revision history + list_revisions but deferred rollback ("schema supports it, needs UX surface").

Fixed: `skills::revert_to_revision` loads historical row, sets it live, appends a NEW revision. History linear. 26th MCP tool `skill_revert {skill_id, rev_seq}`. Same-startup gate. `SkillChanged { kind: "revert" }`. 4 new DAO tests. Operator UI deferred — MCP callable today.

### M13 feat — operator identity on hello reply + admin-only UI gate — 2026-05-15
**Source:** Known limit flagged on #69. PR `<TBD>`.

Was: `OperatorsPanel` + SkillsPanel global toggle gated admin on the server but always visible client-side. Non-admins got silent `forbidden` errors.

Fixed: new `ConsoleOutbound::HelloOk { operator_id, operator_name, role }` emitted after token validation (token not echoed). Frontend reducer populates `state.currentOperator`. `OperatorsPanel` hides for non-admin (hooks called first to preserve React's hook-order invariant).

### M13 feat — is_global toggle in SkillsPanel — 2026-05-15
**Source:** Roadmap polish, finishes the global-skills surface (#68 backend-only). PR `<TBD>`.

Was: #68 added `is_global` + admin-only `skill_set_global` but SkillsPanel had no UI knob.

Fixed: `SkillWithAttachments` + JSON carry `is_global`; `SkillVM` mirrors. Per-row globe toggle button (admin-only on server); 🌐 badge next to the name when set.

### M13 docs — Phase 4 roadmap brainstorm — 2026-05-15
**Source:** Closes the "Phase 4 brainstorm needed" note from the Phase 3 roadmap. PR `<TBD>`.

Was: Phase 3 done end-to-end, every carry-forward landed (including token hashing in #74), but Phase 4 had no spec. Theme E (multi-agent coordination) was deferred from Phase 3 with "needs its own brainstorm before committing."

Fixed: `docs/superpowers/specs/2026-05-15-phase-4-roadmap.md`. Theme E split into E1 (peer review beyond manager), E2 (blocking dependencies + deadlines), E3 (work-stealing among idle peers). Plus F1 (local-LLM smoke), F2 (multi-cliptown federation — XL, may defer), G (operator UX polish bucket). Sequencing recommends F1 first to validate the local-first narrative from #55, then E1→E2→G→E3.

### M13 feat — operator management panel in the console — 2026-05-15
**Source:** Theme B frontend follow-up. PR `<TBD>`.

Was: `operator_list` / `_create` / `_revoke` / `_set_role` ConsoleInbound variants landed in #61 but only via raw WS — no UI surface in the console.

Fixed: new `OperatorsPanel.tsx` mounted below SkillsPanel, collapsed by default. On expand sends `operator_list` to hydrate. Per-row name + role select + Revoke (confirm). Footer creates new operators — on success the freshly-minted `opt_*` token displays in a `MintedTokenBanner` with a copy-now warning that the operator dismisses (cliptown never displays it again). Store reducer handles the four `{type:"ok", kind:"operator_*"}` replies. Non-admin callers see empty list (server's `forbidden` reply is silently swallowed) — explicit role-detect not surfaced yet. Closes the M13 roadmap.

### M13 feat — globally-visible skills — 2026-05-15
**Source:** Roadmap carry-forward (Skills global / non-workspace, M-sized). PR `<TBD>`.

Was: skills strictly startup-scoped — style guides / debug primers had to be duplicated.

Fixed: migration 0008 adds `is_global INTEGER NOT NULL DEFAULT 0` + partial index. `skills::for_agent` UNIONs attached rows with `is_global = 1`; DISTINCT-by-id prevents double-listing. `skills::set_global` DAO. New admin-only ConsoleInbound `skill_set_global {skill_id, is_global}` — manager has no business with world-wide visibility, so `at_least(Admin)`. SkillChanged broadcasts emit `set_global` / `clear_global` kinds. 4 new DAO tests. Agents cannot flag global by design. Frontend UI deferred.

### M13 feat — skills file attachments — 2026-05-15
**Source:** Roadmap carry-forward (Skills file attachments, M-sized). PR `<TBD>`.

Was: skills could only carry a single `content_md` blob — supporting files (templates, JSON configs, examples) had no home. Roadmap listed it as a follow-up.

Fixed: migration 0006 adds `skill_files (id, skill_id FK, name, content, ...)` with `UNIQUE (skill_id, name)` and FK cascade. skills crate gains `upsert_file` / `delete_file` / `list_files` / `file_name_is_valid` (alnum + `- _ .` only — no `..`, no `/`). AttachedSkill gains `files`; `/api/agents/:id/skills` returns the array. Worker materializes each file at `<workdir>/skills/<skill-name>/<file-name>` alongside the main `.md`. 2 new MCP tools (`skill_file_upsert` / `skill_file_delete`). SkillChanged broadcasts emit new kinds `file_upsert` / `file_delete`. 8 new DAO tests. Operator-console UI deferred — agents have MCP path; an operator file editor lands when there's pressure.

### M13 feat — skills revision history — 2026-05-15
**Source:** Roadmap carry-forward (Skills versioning, M-sized). PR `<TBD>`.

Was: every `skills::upsert` overwrote `content_md` in place — no audit, no rollback target. Roadmap listed it.

Fixed: migration 0007 adds append-only `skill_revisions (id, skill_id FK, rev_seq, content_md, created_at, created_by_agent_id?, created_by_operator_id?)` with `UNIQUE (skill_id, rev_seq)` + FK cascade. `skills::Author` enum + `upsert_with_author` record who wrote each version. mcp_dispatch passes `Author::Agent`; cmd_console passes `Author::Operator`. `list_revisions` ownership-gated. New MCP tool `skill_list_revisions {skill_id, limit?}`. 7 new tests. Revision append is best-effort after the live update (history loss < content loss). Rollback deferred — schema supports it but needs UX surface.

### M13 feat — skills content authoring in operator console — 2026-05-15
**Source:** Roadmap carry-forward (Skills content authoring UI, M-sized). PR `<TBD>`.

Was: SkillsPanel only handled attach/detach. Operators had to use MCP tools or raw SQL to create / edit / delete skill content.

Fixed: 2 new ConsoleInbound variants (`skill_upsert_operator` / `skill_delete_operator`), both manager-gated, routing through the same `skills::upsert`/`skills::delete` paths as the agent-side MCP tools. SkillsPanel gains `+ New skill` + per-row ✎ edit / ✕ delete with confirm. The editor starts blank for edit too because the WS snapshot ships skill metadata only — re-fetching content per skill would inflate every snapshot. Operators paste/re-type; upsert resolves by `(startup_id, name)` so the existing row is updated in place. 4 new integration tests.

### M13 feat — cost variance telemetry — 2026-05-15
**Source:** Final Theme C deferred bit (estimate-vs-actual emit). PR `<TBD>`.

Was: Theme C (#53) added `preferred_backend` / `preferred_model` per task but had no closed-loop signal on whether a routing choice was actually saving (or burning) money. CHANGELOG flagged it as deferred pending "estimate inputs first."

Fixed: migration 0005 adds nullable `cost_estimate_usd REAL` to `tasks`. `POST /api/admin/tasks` accepts it (validated finite + non-negative). `cmd_worker::ReportBudget` joins the task row after a successful budget apply; when both estimate and cost are present and the delta crosses ±50%, emits `task_cost_variance` system_event — overrun=warn, underrun=info. 4 new tests cover overrun, underrun, within-threshold (silent), no-estimate (silent). Multi-spawn dedup deferred to the operator console (cliptown-side state is overkill).

### M13 feat — smoke against remote world targets — 2026-05-15
**Source:** Phase 3 Theme A carry-forward (remote-smoke parameterization). PR `<TBD>`.

Was: `scripts/smoke-real-llm.sh` always built + booted its own world and seeded tasks via direct SQL. Remote operators (Fly.io / staging) had no scripted health-check path. The roadmap noted "smoke parameterization for remote targets deferred (manual verification path documented)."

Fixed: new `POST /api/admin/tasks` endpoint (operator-token + manager-role gated) lets clients seed tasks over HTTP — validates startup + assignee, returns the created task. 7 integration tests. Smoke gains `WORLD_REMOTE_URL=https://...` mode: skips cargo build + world boot, derives http+ws bases from the URL, posts to the new admin endpoint instead of SQL. FS-bound + SQL-row checks are skipped in remote mode (no client access to either) — clean adapter exit + responsive `/health` is the remote success signal. `DEPLOY.md` rewritten. Local mode unchanged.

### M13 chore — structured tracing events across hot paths — 2026-05-15
**Source:** Phase 3 Theme D follow-up. `/metrics` (#51) covered the metrics endpoint; the spec's "structured tracing spans through hot paths" stayed deferred. PR `<TBD>`.

Was: only structured signal was `system_events` and `tracing::warn!`/`tracing::info!` ad-hoc calls. No way to bucket dispatch latency or correlate handler activity post-hoc.

Fixed: event-pair tracing (enter + exit-with-elapsed) on `mcp_dispatch::dispatch`, `scheduler::tick`, `cmd_console::dispatch`. Each pair carries kind/identity + corr_id (or tick_seq) + elapsed_us + outcome. Event-pair over Span::entered() because the WS loop's task awaits inside handlers — a `!Send` Span guard breaks `tokio::spawn`'s bound. Default `RUST_LOG=info` sees only error-path; `cliptown_world=debug` surfaces the full pair. Quiet scheduler ticks (no dispatches, <5ms) skip the log.

### M13 feat — admin-only operator management commands — 2026-05-15
**Source:** Phase 3 Theme B follow-up. #52 landed the schema; this PR adds the surface so admins can provision operators without touching SQL. PR `<TBD>`.

Was: Theme B (#52) added the `operators` table + role-aware token validation. The commit message + CHANGELOG explicitly deferred "operator-management commands (provision/revoke/role changes) until multi-operator deploys arrive". TODOS listed it as a Theme B follow-up.

Fixed: 4 new admin-only `ConsoleInbound` variants — `operator_list`, `operator_create`, `operator_revoke`, `operator_set_role`. All gated `identity.role.at_least(Admin)`. `operator_create` mints `opt_<uuid>` token server-side and returns it inline (admin copies once from the response). Self-revoke + self-demotion refused to prevent admins locking themselves out mid-session. Duplicate names → `name_taken`. 8 integration tests cover happy + viewer-rejected + edge cases. TS bindings re-exported. Token hashing deferred — plain bearer is fine until rotation tooling exists. Frontend UI for operator management is a separate task (operator console doesn't yet have a settings panel).

### M13 chore — claude-code adapter honors CLAUDE_CODE_MODEL — 2026-05-15
**Source:** Theme C known-limit follow-up from #58 / #59. PR `<TBD>`.

Was: Theme C wired `preferred_model` → worker → adapter env, but the claude-code adapter didn't surface a model knob (CLI has `--model`; wrapper ignored it). worker's `modelEnvForBackend("claude_code")` returned null and the worker logged + skipped. Theme C wire was 2/3 closed.

Fixed: adapter reads `CLAUDE_CODE_MODEL` from `opts.env`, pushes `--model <id>` onto the CLI args when set. Gated on `useJsonOutput` so the fixture-cli (contract tests) never sees the flag. `modelEnvForBackend("claude_code")` returns `"CLAUDE_CODE_MODEL"`. Worker test updated. Theme C wire 3/3 closed across all adapters.

### M13 feat — per-task worker spawn (Theme C Option B) — 2026-05-13
**Source:** Phase 3 Theme C follow-up #2 — supervisor side. Closes the Theme C wire end-to-end. PR `<TBD>`.

Was: #58 wired the worker to honor `--preferred-backend` / `--preferred-model` flags, but no one passed them. The agent supervisor was per-agent / startup-time-only — it had no knowledge of tasks or their `preferred_*` columns. The Theme C chain stopped at the worker's CLI argv with no caller.

Fixed: opt-in `CLIPTOWN_PER_TASK_WORKERS=1` flips the production path to per-task spawn. With it set: `create_startup` skips daemon spawn; `scheduler::tick` joins `tasks` + `agents` + `startups`, builds a `SpawnConfig { task: Some(TaskSpawn { prompt, preferred_* }) }`, and calls `supervisor.spawn_agent`; `spawn_child` appends `--real --task-id --prompt --preferred-*` when `cfg.task.is_some()`; out_bus liveness check polarity inverts (presence = busy, don't double-spawn); rollback on spawn failure mirrors the existing out_bus failure path. Env var unset keeps the legacy daemon path unchanged so the smoke harness (which sets `CLIPTOWN_TEST_DISABLE_SUPERVISOR=1` and spawns its worker out-of-band) is untouched. 3 supervisor tests (env-var toggle + per-task argv + legacy negative) and 1 scheduler test (env-off fallback). New `fake_worker_dump_args.sh` fixture lets the test assert argv shape without running a real worker. DEPLOY.md secrets section documents the new env var.

### M13 chore — worker honors per-task routing preferences — 2026-05-13
**Source:** Phase 3 Theme C follow-up (worker side of `preferred_backend`/`preferred_model`). PR `<TBD>`.

Was: Theme C (#53) added `preferred_backend` + `preferred_model` columns + forwarded both fields via `TaskAssigned`. Nothing downstream read them — the wire was dead.

Fixed: worker grows two CLI flags `--preferred-backend` / `--preferred-model`. When `--preferred-backend` is set, it overrides `--backend` for adapter selection in `--real` mode. When `--preferred-model` is set, it's forwarded to the resolved adapter via its model env var (codex → `CODEX_MODEL_ID`, opencode → `OPENCODE_MODEL`). New `modelEnvForBackend()` helper exported + unit-tested. claude_code returns null today — the adapter doesn't thread a model knob (CLI has `--model`, the wrapper doesn't expose it); flagged as a known limit. 5 new tests in `main_args.test.ts`. Agent supervisor still per-agent-default — next wiring step is to extend `SpawnConfig` + supervisor spawn path so the world auto-injects `--preferred-*` based on the dispatched task's row.

### M13 chore — execenv GC script — 2026-05-13
**Source:** Phase 3 roadmap carry-forward #2 (Execenv GC daemon). PR `<TBD>`.

Was: per-task execenv workdirs at `workspaces/<sid>/<tid>/workdir` accumulated forever — operators had no GC recipe shorter than `rm -rf`. Disk slowly filled on long-running deployments.

Fixed: `scripts/gc-execenv.sh` (bash + sqlite3). Selects tasks in terminal states (`done` / `failed` / `escalated`) AND `updated_at` older than `--days N` (default 7), removes their workdir. Artifacts dir preserved so audit replays still work. Read-only SQL access — safe to run while world is up. `--dry-run` + `--db` / `--workspaces` overrides for docker / Fly.io layouts. Smoke-tested locally against a 4-row fixture covering all four cases (terminal+old reaped, in_progress+old kept, terminal+recent kept, terminal+old-but-missing counted). Operator recipe added to `docs/OPERATOR.md`. World-side periodic auto-GC deferred — explicit operator-run is safer; promote to a scheduler task if it gets tedious.

### M13 chore — bench gate flipped + CI baselines recalibrated — 2026-05-13
**Source:** Phase 3 roadmap carry-forward #1 (bench.yml hard-gate flip). PR `<TBD>`.

Was: `bench.yml` ran `continue-on-error: true` so regressions never failed PRs. Baselines were Apple Silicon numbers (~3x off ubuntu CI), and on top of that the bench compile had been broken since Phase 3 Theme B (#52) — `Cmd::HandleConsoleMsg` gained an `identity` field but `crates/world/benches/world_bench.rs` was never updated. Bench was effectively dead since #52.

Fixed: bench compile patched (`OperatorIdentity::admin_for_tests()` passed through). `bench/baselines.json` v3 with CI-grade numbers (72 µs tick, 361k msgs/s throughput) averaged from 3 recent successful CI runs, with `_ci_samples_*` fields preserved for future re-baseline reference. `continue-on-error: true` dropped — bench regressions now fail the PR. Tolerance stays at 20%; widen the baselines (not the tolerance) if it starts flapping.

### M13 docs — local-first deploy + local LLM routing — 2026-05-13
**Source:** Post-Phase-3 docs follow-up triggered by "로컬 LLM을 사용하려면 로컬 실행이 더 좋을 것 같다" framing question. PR `<TBD>`.

Was: `docs/DEPLOY.md` led with Fly.io and `README.md` Deploy section pointed straight at cloud. Cloud VM can't reach a local GPU, so for the most interesting LLM workflow (ollama / vLLM / LM Studio on the dev's own box) the guide was actively misleading.

Fixed: DEPLOY.md restructured to native → docker compose → **local LLM (new section)** → Fly.io → other targets. New "Local LLM (ollama, etc.)" section documents how the codex / opencode adapters propagate `OPENAI_BASE_URL` + model env vars to the spawned CLI (`...process.env` spread that was always there but undocumented). claude-code + local backend flagged as needing a translator proxy. Vercel added to "doesn't fit" list alongside Cloud Run. README Deploy paragraph reordered. No code change — purely making the existing local-LLM path discoverable.

### M13 Phase 3 Theme C — per-task routing preferences — 2026-05-13
**Source:** Phase 3 roadmap Theme C. PR `#53`.

Was: tasks were routed implicitly to whatever backend/model was provisioned on the agent at startup. No way to opt a single subtask into a cheaper model (haiku for trivial work, opus for the heavy reasoning step) without re-provisioning the agent.

Fixed: migration 0004 adds nullable `preferred_backend` + `preferred_model` to `tasks`. `WorkerOutbound::TaskAssigned` (ts-rs auto-export) carries both. Scheduler reads the row and forwards them on dispatch. New 22nd MCP tool `task_set_preference` (manager-or-assignee gated, cross-startup blocked, audit row + `task_routing_changed` system_event). 4 new MCP handler tests + 1 scheduler propagation test. Worker-side adapter honoring documented in CHANGELOG carry-forward — the field arrives on `task_assigned`; the adapter spawn path will start preferring it once the budget-routing UX lands. Cost variance telemetry (estimate-vs-actual emit) deferred — needs estimate inputs.

### M13 Phase 3 Theme B — operator RBAC — 2026-05-13
**Source:** Phase 3 roadmap Theme B. PR `#52`.

Was: console access went through a single shared `CLIPTOWN_OPERATOR_TOKEN` env var. No notion of operator identity, no role separation — anyone with the token could force-accept tasks and attach skills. Audit log recorded "operator" as a faceless actor.

Fixed: new `operators` table (migration 0003) maps bearer tokens to `(id, name, role)` with role ∈ {viewer, manager, admin}. `auth.rs::validate_operator_token` returns typed `OperatorIdentity` (table-first with env-var fallback for backward compat — env-var path synthesizes an admin identity so dev workflows survive). Identity propagates from WS-hello through `Cmd::HandleConsoleMsg` into `cmd_console::dispatch`, which gates each `ConsoleInbound` arm: viewer-level for read-ish ops (possess/unpossess/move/recheck-backends/hello), manager-level for everything that writes to SQL or fans broadcasts (directive, accept/reject proposal, force-accept/fail, skill attach/detach). Forbidden returns `{"type":"error","reason":"forbidden"}` before any side effect. 3 new integration tests cover the gating + 4 unit tests on the validator. Admin-only operator-management commands (provision/revoke/role-change) deferred — schema is in place; the inbound surface arrives with multi-operator deployments.

### M13 Phase 3 Theme D — observability (/metrics) — 2026-05-13
**Source:** Phase 3 roadmap Theme D. PR `#51`.

Was: only structured signal was `system_events` in SQL. No metrics endpoint for external scrapers; no easy liveness/load visibility.

Fixed: new `crates/world/src/metrics.rs` hand-rolled Prometheus text exposition + `/metrics` HTTP route. Atomic global counters increment from `mcp_dispatch` per call + per error. Per-scrape gauges from SQL + WorldView: active startups, per-startup budget, tasks-by-status (8 labels), agents-by-health-bucket (4 labels), `tick_seq` for loop liveness. 2 new unit tests (rust 248). Structured tracing spans deferred — add piecemeal as hotspots emerge.

### M13 Phase 3 Theme F — documentation pass — 2026-05-13
**Source:** Phase 3 roadmap Theme F. PR `#50`.

Was: contributor + operator docs were sparse. README focused on stale Phase 0 details; no ARCHITECTURE, no OPERATOR, no AGENT guide. Onboarding required code-reading.

Fixed: README rewrite reflecting Phase 0-2 sealed + Phase 3 underway with updated test counts and new "Real-LLM smoke" + "Where things live" sections. New ARCHITECTURE.md with topology diagram + component walkthrough + 21-tool MCP catalog summary + invariants. New docs/OPERATOR.md covering console UX (possess / kanban / skills / directives / budget / system events / SQL recipes). New docs/AGENT.md covering the adapter-CLI POV (workdir layout, CLAUDE.md contract, all 21 MCP tools categorized, hooks, budget, sandbox, common patterns).

### M13 Phase 3 Theme A — production deploy story — 2026-05-13
**Source:** Phase 3 roadmap Theme A (from `docs/superpowers/specs/2026-05-13-phase-3-roadmap.md`). PR `#49`.

Was: cliptown was single-machine dev-friendly only. No Docker, no cloud deploy story, no secrets pattern. Operators couldn't run it for real workloads.

Fixed: `Dockerfile` (multi-stage rust + node bundle) + `docker-compose.yml` (local-prod equivalent with persistent volumes) + `fly.toml` + `docs/DEPLOY.md` covering quickstart, Fly.io walkthrough, secrets pattern, rotation, rollback, and sketches for AWS/GCP/K8s/VPS. `README.md` gains a Deploy section. Verified locally: image builds, container boots, `/health` returns `{"ok":true}`. Smoke parameterization for remote targets deferred (manual verification path documented).

### M12 P2.2 skills broadcasts + UI — 2026-05-13
**Source:** P2.2 known-limitation cleanup. PR `#46` (follow-up to #43).

Was: P2.2 MVP shipped DAO + MCP tools + worker integration but left no operator console UI and no `skill_changed` broadcasts — operators had to use SQL or MCP tools blind.

Fixed: 2 new ConsoleOutbound variants (SkillChanged on every mutation, SkillsSnapshot at connect) + 2 new ConsoleInbound (SkillAttach/Detach via operator). All 5 MCP skill handlers emit broadcasts. New `SkillsPanel.tsx` in the operator console: read view + attach dropdown + detach chips. Content authoring (create/edit/delete) still on MCP for now (heavier editor deferred).

### M12 P2.2 skills system — 2026-05-13
**Source:** Phase 2 backlog last item (from `docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md` § P2.2). PR `#43`.

Was: cliptown agents saw only `task.title` + `task.description`. No way to compound reusable capability — every new agent session reinvented the wheel.

Fixed: per-startup markdown skills attached many-to-many to agents. SQL: `skills` + `agent_skills` tables (migration `0002_skills.sql`). World: `crates/world/src/skills.rs` DAO + 5 MCP tools (`skill_upsert` / `skill_list` / `skill_attach` / `skill_detach` / `skill_delete`) + HTTP endpoint `GET /api/agents/:id/skills`. Worker: `skills_client.ts::fetchSkillsForAgent` + `prepareWorkdir` extension writes each attached skill as `<workdir>/skills/<name>.md` and adds an "Available skills" section to CLAUDE.md. Smoke seeds a skill + verifies on-disk landing. Frontend UI / `skill_changed` broadcasts / global skills / file attachments / versioning all deferred (Known limitations).

### M12 P2.3 per-task execenv directories — 2026-05-13
**Source:** Phase 2 backlog second item (from `docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md` § P2.3). PR `#41`.

Was: worker passed a flat `--workspace` arg to every adapter spawn — every task on the same agent shared the same filesystem context, with no place to inject per-task context files or skill content. This blocked P2.2 (skills) and made "many tasks per agent" hostile to isolate.

Fixed: new `packages/worker/src/execenv.ts::prepareWorkdir` creates `<workspaces_root>/workspaces/<sid>/<tid>/workdir/` per task, with an absolute symlink `workdir/workspaces` → `<workspaces_root>/workspaces` so the agent's existing relative artifact path resolves through the symlink to the canonical location (no prompt or world change). A minimal `CLAUDE.md` lands in the workdir at spawn carrying agent_id / task_id / startup_id + the canonical artifact path contract. Worker's `--task-id` is now required. GC daemon deferred — known limitation in CHANGELOG.

### M12 P2.1 daemon health buckets — 2026-05-12
**Source:** Phase 2 backlog first item (from `docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md` § P2.1). PR `#39`.

Was: cliptown's operator console treated worker liveness as binary (WS connected vs closed). A 5-minute network blip looked identical to a hard crash, generating noise.

Fixed: `AvatarView` now carries `last_seen_at: Option<i64>` (updated on RegisterWorker / HandleWorkerMsg) and `health: Health` (derived per tick from connection state + age of last_seen). 4 states — `online` / `recently_lost` / `offline` / `about_to_gc` — replace the binary signal. New `crates/world/src/health.rs` pure module owns derivation + thresholds. Frontend `AvatarVM` mirrors the shape; Pixi alpha dims non-online avatars. 11 new tests (8 unit + 3 integration).

### M11 real bench harness — 2026-05-12
**Source:** Phase 1 known-limitation cleanup. PR `#37`.

Was: `crates/world/benches/world_bench.rs` shipped Phase 0 with two placeholder benches — `tick_latency_per_loop_iter` ran `sum 0..1000` inside a tokio runtime; `mpsc_throughput_1k_msgs` measured a generic in-process mpsc channel. Neither touched real world code.

Fixed: both benches now drive a real `loop_::spawn` handle. `tick_latency_real_loop` measures one `Cmd::Tick` round-trip end to end; `console_dispatch_throughput_100_msgs` fires 100 `Cmd::HandleConsoleMsg` through the same dispatcher lane real console commands use. `bench/check.mjs` learned the `100_div_median_us` extract recipe; `bench/baselines.json` carries fresh medians captured on the dev box. The Phase-1 known-limitations bullet about placeholder benches retires.

### M11 hook bridge parity — codex + opencode — 2026-05-12
**Source:** Phase 0 known-limitation (`#31` follow-up). PR `#36`.

Was: codex + opencode adapters advertised `[session_stop, session_error]` capabilities but no hook events actually flowed. Each spun up an HTTP `startHookBridge` listener and exposed it via `CODEX_HOOK_PORT` / `OPENCODE_HOOK_PORT`, but nothing on the upstream CLI side ever POSTed to it — dead weight.

Fixed: codex now drives `opts.onHook` from a streaming JSONL parser over `codex exec --json` stdout (`event_parser.ts`); opencode was rebuilt around `opencode serve --port 0 --pure` + `/event` SSE so we observe `pending → running → completed` state transitions for true pre/post semantics (`event_mapper.ts` + `sse_client.ts` + `serve_lifecycle.ts` + `session_client.ts`). Dead HTTP bridge removed from both adapters; `adapter-core/hook_bridge.ts` kept for claude-code. Three smoke-discovered fixes shipped in the same PR: claude CLI 2.1.x needs `--settings <path>` (env var ignored), `opencode serve` emits listening URL on stdout not stderr, and the shared bridge now reads `tool_name` first (claude payload shape) before `tool`. Capability advertising on all three adapters now matches reality. § 11.9 smoke verified named-tool hook lines on each backend (claude `Write` / `mcp__cliptown__task_done`, codex `shell` / `task_done`, opencode `apply_patch` / `cliptown_task_done`).

### Body-length validation on chat/directive (P2) — 2026-05-11
**Source:** Codex adversarial review on M5 ship (P2 #1)

Was: workers could send unbounded `body` via `speak`, managers could send unbounded `feedback` via `task_request_changes`, and operators could send unbounded `body` via `OperatorDirective`. Each cloned the full string into the broadcast channel (capacity 4096, Lagged-fatal-close), the SQL `messages` row, and the frontend's 500-entry messages array — a chatty / malicious agent or operator could starve the operator console by pushing real events out of the buffer.

Fixed: `mcp_dispatch::MAX_BODY_LENGTH = 4096` (chars) + `check_body_length` helper guarding the three producer call sites pre-side-effect. Workers see `mcp_error{code:"body_too_long"}`; operators see `error{reason:"body_too_long"}`. Regression guards: `speak_rejects_body_too_long`, `speak_accepts_body_at_cap`, `task_request_changes_rejects_feedback_too_long`, `no_broadcast_on_body_too_long`.

### `emit_system_event` silent JSON fallback on malformed payload (P3) — 2026-05-11
**Source:** Codex adversarial review on M5 ship

Was: `emit_system_event` wrote the raw payload string to SQL but used `serde_json::from_str(payload).unwrap_or(Value::Null)` for the broadcast frame. SQL row had the raw string, broadcast frame had `Value::Null` — operator console and audit log diverged on malformed input.

Fixed in `crates/world/src/emit.rs`: parse via `match` and log `tracing::error!` on failure, then send the raw string as `Value::String(raw)` on the wire so SQL and broadcast carry identical data. Loud-fail surfaces the producer bug to operators instead of silent null-degradation. Regression guard: `console_emit::emit_system_event_malformed_payload_preserves_raw_on_broadcast`.
