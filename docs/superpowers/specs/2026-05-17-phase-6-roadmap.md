# Phase 6 — roadmap brainstorm

**Date:** 2026-05-17
**Status:** brainstorm — not a binding spec, just direction.
**Driver:** Phase 5 sealed (#86 → #91, with #92 lockfile fix +
v0.1.0 tag). Cliptown is now self-hostable for a 2-5 operator
team. Coordination is solid. The interesting Phase 6 question
is what to do about *agent quality* — the agents themselves
producing better work, not just being better-coordinated.

## Where we are

- **Phase 4** shipped peer review, blocking deps + deadlines,
  async work-stealing, and the kanban-polish bucket.
- **Phase 5** shipped operator presence, per-operator audit
  visibility, soft-locks on destructive actions, observability
  artifacts (Grafana + Alertmanager), the docker deploy
  pipeline (compose + GHCR release.yml), and the SQLite
  hot-snapshot + restore drill.
- **v0.1.0** tagged today, exercising the GHCR release
  pipeline end-to-end for the first time.
- **Deferred-but-named from prior phases:** F2 federation
  (XL), public SaaS, corp SSO.
- **Pain that the small-team coordination work didn't
  touch:** engineers submit `task_done` with broken
  artifacts (failing tests, lint errors, missing files);
  managers spend LLM cycles bouncing the work back; the
  cycle repeats until `max_review_rounds` escalates the
  task to operator. Self-review is missing.

The framing for Phase 6 is "make the engineer agent submit
work that's correct on the first try more often, and recover
gracefully when it isn't." Three themes, sequenced A→B→C, all
focused on closing that loop without dragging in memory or
decomposition (Phase 7 candidates).

## Phase 6 candidate themes

Sorted by sequencing dependency. Each theme leans on the
artifacts the previous theme ships.

### Theme P6.A — Self-review gates (recommended first)

**Why:** Today the engineer agent calls `task_done` and the
work goes directly to manager review. Bad artifacts (failing
tests, lint errors, missing artifact_path, schema violations)
ride straight to the manager queue, eating manager LLM cycles.
A pre-submit self-check shifts the easy catches off the
manager — even a thin check (file-existence + markdown lint)
catches the most common mistakes.

**Scope:**
- New MCP tool `self_review {task_id, artifact_path}`. Runs
  a configurable check pipeline server-side:
  - artifact-exists (matches the canonical
    `workspaces/<sid>/artifacts/<tid>.md` pattern + is
    present + non-empty).
  - markdown lint (existing `verify` stub `lint_markdown`
    wired up to actually run).
  - JSON lint when artifact extension is `.json` (already
    works in `verify`).
  - TS/Rust lint (deferred to P6.B; gate returns a
    "deferred-stub" for these extensions until B lands).
  - Optional skill_file-defined assertions (e.g. an attached
    skill ships a YAML of regex/exact-match assertions
    against the artifact).
- Return shape: `{ok: true}` on pass, `{ok: false, must_fix:
  [{check, severity, message}]}` on fail. `severity` is
  `warn | error`; `error` blocks `task_done`'s status flip.
- `task_done` gains optional `auto_check: bool` field
  (default true). When true, `task_done` runs `self_review`
  as part of the transition and returns the must_fix list
  instead of flipping to `awaiting_review` on failure.
- New `tasks.self_reviewed_at INTEGER` timestamp + audit
  trail entry `{actor:"engineer", kind:"self_review",
  outcome:"pass"|"fail", checks:[...]}`.
- Migration 0014 (current latest is 0013, P5.C `action_locks`)
  for the new column.

**Size:** M (one PR). New MCP tool + SQL field + audit +
tests. The `verify` machinery exists already; this PR wires
it to a structured pre-submit pipeline.

**Closes:** "manager has to catch every broken artifact" pain.

### Theme P6.B — Structured tool surface for self-review

**Why:** P6.A's gate can do file-existence + light markdown
lint, but real checks (run tests, lint TS/Rust, diff against
a base ref) need shell access. Agents have full CLI access
via adapter spawn today, but cliptown has no *structured*
tool surface that records the run, so self-review's verdict
isn't auditable and the model cascade in P6.C can't reason
about "what's already been tried."

**Scope:**
- New MCP tool `run_tests {task_id, command?}`. Runs the
  agent's preferred test command (e.g. `pnpm test`,
  `cargo test`) inside the per-task execenv (P2.3 sandbox).
  Captures exit code + the tail of stdout/stderr (cap the
  body at `MAX_BODY_LENGTH` to keep audit rows bounded).
  When `command` is unset, falls back to a per-startup
  default or sniffs the workspace for known config files
  (Cargo.toml → `cargo test`, package.json → `pnpm test`).
- New MCP tool `lint_artifact {task_id, artifact_path}`.
  Extends the existing `verify` deferred-stubs to actually
  invoke linters:
  - `.ts` / `.tsx` → `tsc --noEmit` on the file.
  - Rust workspace (any `.rs` files in the per-task
    execenv) → `cargo check --message-format=short`.
  - `.md`, `.json` → already work via the existing
    `verify` paths; this tool just exposes them via a
    dedicated MCP entry that emits an audit row.
- New MCP tool `read_artifact_diff {task_id, artifact_path,
  base_ref?}`. Returns the git diff between the artifact
  and a base ref (defaults to HEAD before the task started
  — captured at dispatch time on `tasks.base_git_ref`).
  Useful for self-review (large diff → flag for human
  review) and for peer review (P4 Theme E1).
- Each tool emits a structured audit_trail entry so the
  operator console (and the P6.C recovery pass) can see
  exactly what was run, when, and the outcome.
- Sandboxing reuses `crates/world/src/sandbox.rs` + the
  per-task execenv from P2.3. Exec timeout cap; per-call
  cost budget (refuse if startup is paused at 100%).

**Size:** M-L (one PR but heavy on sandbox integration +
multi-language linter wiring). May need to split out the
Rust linter path if it lands clean.

**Closes:** "self-review can only do trivial checks" gap.

### Theme P6.C — Auto-recovery on review failure

**Why:** Today, when a task fails self-review (P6.A) or
manager review N times, the only paths are operator
force-fail or wait for `max_review_rounds` to escalate to
the operator. We can do better — the same task may succeed
with a different model tier, a different agent, or a
combination. Auto-recovery composes the cascade before
giving up.

**Scope:**
- Migration 0015 adds
  `startups.auto_recovery_enabled INTEGER NOT NULL DEFAULT 0`
  + `startups.auto_recovery_max_attempts INTEGER NOT NULL
  DEFAULT 2`.
- New scheduler post-dispatch pass (mirrors P4 Theme E3's
  auto-steal pass): for each task with `review_round >=
  auto_recovery_threshold` (computed per startup), pick a
  recovery strategy:
  1. **Model cascade.** If `preferred_model` is unset or is
     at a lower tier than the catalog's max, bump to the
     next tier. Re-dispatch the task with the new
     `preferred_model`. Audit `kind=auto_recovery,
     strategy=model_cascade, from=…, to=…`.
  2. **Peer reassignment.** If model cascade is exhausted,
     pick an idle same-role peer (reuses the SQL plumbing
     from P4 E3 `task_steal`). Audit
     `strategy=peer_reassign, from=…, to=…`.
  3. **Escalate.** Both above exhausted → existing
     `task_escalated` path. No auto-recovery flag set →
     same path (current default behavior).
- Each successful recovery emits a `task_recovered`
  system_event with severity `info`. The operator console
  surfaces it as a transient toast (Theme G slice 1
  surfacing pattern reused).
- Admin-only `StartupSetAutoRecovery {startup_id,
  enabled, max_attempts?}` ConsoleInbound + handler. UI
  toggle in MainHeader next to the auto-steal pill (P5.B
  AgentsPanel pattern).

**Size:** M (migration + scheduler pass + ConsoleInbound +
admin UI toggle + tests).

**Closes:** "failed tasks sit in escalated until a human
intervenes" gap. Bakes in once the team is using cliptown
for real work and wants self-healing.

## Recommended sequencing

1. **First**: P6.A (self-review gates). Lands as a thin gate
   (file-existence + markdown/JSON lint via existing
   `verify` machinery). Every catch shifts work off the
   manager from day one, even before B's heavier tooling.
2. **Second**: P6.B (structured tools). Immediately
   upgrades the gate from "trivial checks only" to "real
   verification." Doing B before A would mean shipping
   tools nothing calls; doing A first then B is the right
   invariant.
3. **Third**: P6.C (auto-recovery). Composes A and B —
   the recovery pass reads the audit trail B writes to
   decide what's already been tried. Doing C without A
   means we recover on the wrong signal (raw
   manager-review-round count vs. self-review-aware
   count); doing C without B means model cascade fires
   blind without knowing whether tests pass.

## What this doc is NOT

- **Not memory.** Per-task scratchpad + per-agent long-term
  memory + RAG retrieval is the largest Phase 7 candidate.
  P6.B's `read_artifact_diff` gives some context-on-retry
  but real memory is its own phase.
- **Not decomposition.** `subtask_create` stays operator-
  /manager-driven. LLM-assisted decomposition is a Phase 7
  theme if real demand surfaces.
- **Not prompt templates.** Manager directives stay
  free-form. Templated prompts are high-maintenance,
  low-novelty; not a phase shaper.
- **Not new agent roles.** founder/manager/engineer/
  designer stay as-is. Persona work is Phase 7+ territory.
- **Not federation (F2).** Still deferred from Phase 4.
  Still XL. Still pending real demand.
- **Not corp SSO / public SaaS.** Same frame as Phase 5 —
  small-team self-hosted, not multi-tenant product.

## Suggested next concrete action

Pick **P6.A** (self-review gates) for the first Phase 6 PR
cycle. Smallest scope, immediate quality gain even with thin
checks, lands the `self_review` MCP tool + `task_done
auto_check` plumbing that B will fill in.

Open question to resolve at brainstorm time: when
`self_review` fails with `severity:"warn"` (not `error`),
does it block `task_done` or just annotate the audit row?
Default position is "warnings don't block" — overrideable
per-startup if real friction shows.
