# Phase 3 — roadmap brainstorm

**Date:** 2026-05-13
**Status:** brainstorm — not a binding spec, just direction
**Driver:** Phase 0 (bring-up), Phase 1 (perf gate + adapter parity), and Phase 2 (multica patterns — health buckets / execenv / skills MVP + UI polish) all sealed across 12 milestones (#33 → #47). cliptown now does the full real-LLM round trip with per-task isolation, attached skills, and an operator console reflecting live state. This doc maps the candidate Phase 3 themes, sizes them, and recommends a sequencing.

## Where we are

- **Test totals on main:** rust 246 / worker 75 / adapter-* 35 / frontend e2e 16 / bench gate ok.
- **Spec invariants:** all 9 from the original cliptown-design spec proven at rust + UI.
- **Real-LLM ship-gate (§ 11.9):** verified end-to-end against claude-code, codex, and opencode CLIs. Budget telemetry forwarded.
- **Operator console:** Pixi 2D canvas + kanban + chat + skills panel + system event feed. Possess transitions. ~5 console-side product surfaces.
- **Phase 2 multica patterns:** all three landed (P2.1 health buckets, P2.3 execenv, P2.2 skills MVP + UI).

The "minimum viable cliptown" exists. Phase 3 is about productionizing + extending capability surface, not closing fundamental gaps.

## Carry-forward limitations (housekeeping)

Small items still in CHANGELOG known-limitations. Should be cleared as opportunistic chore PRs, not Phase 3 themes:

1. **`bench.yml` hard-gate flip** — XS. Requires recalibrating ubuntu CI baselines first (dev box values diverge). Capture 5+ CI samples → average → write to baselines.json → flip `continue-on-error: false`.
2. **Execenv GC daemon** — S, ~3-4h. Standalone `scripts/gc-execenv.sh` or world-side periodic task. Walks `workspaces/<sid>/<tid>/` dirs older than 7 days and removes them.
3. **Skills content authoring UI** — M, ~1 day. Inline content_md editor + create/delete in SkillsPanel. Heavier UX than read+attach.
4. **Skills file attachments** — M. Schema change for attached files array, worker writes files into `<workdir>/skills/<name>/`.
5. **Skills global (non-workspace)** — M. Schema migration + permissions.
6. **Skills versioning** — M. Audit log + revision history.

Total housekeeping budget: ~1 week if all six land. Or one S+XS chore PR at a time, interspersed with Phase 3 themes.

## Phase 3 candidate themes

Ordered by my read of strategic value × tractability.

### Theme A — Production deploy story (recommended first)

**Why:** cliptown is single-machine dev-friendly today. Operators can't run it for real workloads without solving image/runtime/secret/persistence storyline. Without this, Phase 3 capability work has no deployment target to validate against.

**Scope:**
- Dockerfile for world server (multi-stage: rust build → slim runtime)
- Dockerfile for worker
- `docker-compose.yml` for local-prod equivalent (world + sqlite volume + reverse proxy)
- Deploy guide for Fly.io (single VM + persistent volume) — the simplest cloud target
- Secrets pattern: env-var injection at boot, no committed secrets, document operator-token + agent-secret rotation
- Smoke against deployed instance (parameterize `WORLD_BIND` + auth tokens)

**Size:** L, ~3-4 days. Mostly bash + YAML; little Rust.

**Closes:** "How do I run cliptown for real?" — the implicit question every README reader has.

### Theme B — Operator RBAC + audit log polish

**Why:** Today there's one operator token. Real deployments need multiple operators with different scopes (read-only vs full control), an audit trail of operator actions, and the ability to rotate / revoke.

**Scope:**
- New `operators` SQL table with `(id, name, token_hash, role, created_at)`. Role ∈ {viewer, manager, admin}.
- Middleware that maps tokens → operator + role, gates ConsoleInbound variants by role.
- `system_events` already exists; extend to include the operator_id source for any operator-initiated event.
- Console UI: show which operator is connected (top bar), audit log viewer panel.
- Migration path: existing `dev-token` becomes seeded admin.

**Size:** M, ~2-3 days.

**Closes:** "Can multiple people share a deployment without stepping on each other?"

### Theme C — Cost optimization & model routing

**Why:** Budget ladder works but every spawn picks the configured backend statically. Some tasks are cheaper on haiku, some need opus. Routing decisions made before spawn could save 30-70% on a real workload.

**Scope:**
- Per-task `preferred_backend` / `preferred_model` SQL field (nullable; falls back to startup default).
- Scheduler picks backend based on (task tags, estimated complexity, current spend, cap distance).
- New world MCP tool `task_set_preference` for managers to set per-task overrides.
- Telemetry: emit per-task cost variance vs estimate to system_events.

**Size:** M-L, ~3-5 days. Some heuristic design; rust + frontend work.

**Closes:** budget realism for non-trivial workloads.

### Theme D — Observability layer

**Why:** `system_events` is the only structured telemetry today. For real ops, metrics endpoints (prom-style) + structured log levels + tracing spans would dramatically reduce time-to-diagnose.

**Scope:**
- `/metrics` Prometheus endpoint on world server: active startups, tasks by status, budget per startup, agent health bucket counts, MCP tool call rates.
- Structured `tracing` spans through the hot paths (MCP dispatch, scheduler tick, view broadcast). Existing `tracing::warn!` calls become spans.
- Operator console: alert routing panel (route system_event by severity → toast / email / no-op).

**Size:** M, ~2-3 days.

**Closes:** "Is cliptown healthy right now?"

### Theme E — Multi-agent coordination richer than M5

**Why:** Current task graph supports parent-child + review cycle. Real teams need: cross-startup collaboration (rare; out of scope?), peer review beyond manager review, time-bounded blocking dependencies, async work-stealing among idle peers.

**Scope:** large; needs its own brainstorm before committing.

**Size:** XL, ~1-2 weeks per sub-feature.

**Closes:** more realistic team simulations. Maybe over-scoped for Phase 3; defer to Phase 4.

### Theme F — Documentation pass

**Why:** README, ARCHITECTURE, agent operator guide, CLI/spec docs are stale-ish or absent. Onboarding a new contributor or operator takes too much code-reading.

**Scope:**
- README rewrite: clear "what is cliptown" + 30-min quickstart + how to run § 11.9 smoke.
- ARCHITECTURE.md: world ↔ worker ↔ adapter diagram, MCP tools catalog (auto-generated from `handle_tools_list`).
- OPERATOR.md: how to manage skills, tasks, budgets, agents from the console.
- AGENT.md: what cliptown looks like from an adapter-spawned CLI's POV (CLAUDE.md contract, MCP tool surface, hook events).
- Cross-link to `docs/superpowers/specs/` for deep-dives.

**Size:** M, ~2 days. No code.

**Closes:** "How do I read this codebase?"

## Recommended sequencing

1. **First**: Theme A (Production deploy) — unblocks all subsequent work having a real deployment target.
2. **Second**: Theme F (Documentation) — small, high-leverage, unblocks contributors.
3. **Third**: Theme D (Observability) — needed for any real production run.
4. **Fourth**: Theme B (RBAC + audit) — gates multi-operator scenarios.
5. **Fifth**: Theme C (Cost optimization) — capability surface; non-blocking but high ROI.
6. **Sixth (separate brainstorm)**: Theme E — too big for this phase, separate spec needed.

**Carry-forward housekeeping**: thread in opportunistically between themes. Each chore PR is XS-S; total ~1 week if all six land.

## What this doc is NOT

- Not a binding plan. Each Theme should get its own brainstorm spec when scheduled.
- Not a deadline / commitment. Themes are sequenced for value, not time-boxed.
- Not exhaustive — Theme E acknowledges that more multi-agent richness is its own deep area.

## Suggested next concrete action

Pick **Theme A** (Production deploy story) for the next brainstorm cycle. It unblocks the rest and has a tractable single-PR-per-major-task shape:

- PR-1: Dockerfile + docker-compose
- PR-2: Fly.io deploy guide + deploy script
- PR-3: Secrets / token rotation docs + smoke parameterization

That's a 3-PR sequence that lands a real deployable cliptown. Open question: do we target a different cloud (AWS / Render / Railway) instead of / in addition to Fly.io? Worth resolving at brainstorm time.
