# TODOS

## Open

_(empty)_

## Completed

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
