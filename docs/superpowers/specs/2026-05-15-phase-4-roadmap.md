# Phase 4 — roadmap brainstorm

**Date:** 2026-05-15
**Status:** brainstorm — not a binding spec, just direction.
**Driver:** Phase 3 sealed (#49 → #74). 5 themes shipped + 13
carry-forward follow-ups. Token hashing closed today (#74). Theme E
(richer multi-agent coordination) was deferred from Phase 3 with
"needs its own brainstorm before committing" — this doc is that
brainstorm, plus everything else Phase 4 might reasonably take on.

## Where we are

- **Test totals on main (post-#74):** rust ~290 / worker 87 /
  adapter-* 35 / frontend e2e 16 / bench hard-gate ok.
- **Phase 3 themes** (#49–#69): production deploy / docs / observability
  /metrics / RBAC / cost optimization & per-task routing /
  documentation pass. All landed + UX-completed.
- **Carry-forwards** (#70–#74): is_global UI, hello identity gate,
  revision rollback, execenv auto-GC daemon, operator token
  hashing. All closed in this push.
- **Local-first deploy** (PR #55): the headline shipping mode for
  cliptown is now `pnpm dev` + ollama, not Fly.io. Cloud is a
  secondary "share with collaborators" target.
- **Per-task worker spawn** (#59) wires Theme C to production-live;
  opt-in via `CLIPTOWN_PER_TASK_WORKERS=1`.

Cliptown's "team-of-agents simulation against a real LLM" loop is
done. The interesting question for Phase 4 is what to layer on top
of it.

## Phase 4 candidate themes

Sorted by my read of strategic value × tractability.

### Theme E1 — Peer review beyond manager review (recommended first)

**Why:** Today every review is `manager → engineer`. Real teams do
peer review: a designer reviews engineer output, an engineer
reviews founder spec. Adds a different shape of supervision and
covers cases where the manager isn't the right reviewer.

**Scope:**
- Extend `task_request_changes` to accept a `reviewer_agent_id`
  field so the caller isn't structurally tied to manager_id.
- New `peer_review_pool` per startup: a set of agent ids eligible
  to review tasks they didn't author. Stored on `agents` or
  per-task assignment.
- Console drag-drop or MCP-side gesture for "ask peer X to
  review."
- Audit: separate `peer_reviewed_by` field on task audit_trail
  events so the org graph stays inferable.

**Size:** M, ~2-3 days.

**Closes:** "Why is the founder the only one who can sign off?"

### Theme E2 — Time-bounded blocking dependencies

**Why:** Today subtasks block their parent (`parent_id`) by
existing — there's no notion of "this task is blocked on agent
B's task X, with a deadline." Real coordination needs explicit
deadlines + escape hatches.

**Scope:**
- New `tasks.blocked_on` (FK to another task) + `tasks.deadline_at`.
- Scheduler refuses to dispatch a `blocked_on` task until the
  dependency hits a terminal state; emits `task_unblocked` event
  when the chain clears.
- Deadline crossing emits a `task_overdue` system_event. The
  blocking agent's manager (or peer pool, see E1) auto-receives a
  directive to triage.
- Console kanban swimlane: "blocked" column with hover-shows-on-what.

**Size:** M, ~2-3 days.

**Closes:** "Agent A is stuck because B's prerequisite never
finished and nobody noticed."

### Theme E3 — Async work-stealing among idle peers

**Why:** Today an idle engineer just sits there even when another
engineer in the same startup has 4 queued tasks. Cliptown's pool
of engineers is a labor market — let idle hands grab work.

**Scope:**
- New ConsoleInbound + MCP `task_steal {task_id}` (idle agent
  claims a queued task from another agent in the same startup,
  same role).
- Heuristic auto-steal: scheduler offers stealable tasks to idle
  same-role peers after N idle ticks. Configurable per startup.
- Audit: `actor: stealer` on the assignee change event.
- Operator console toggle per startup: "allow auto-steal" boolean.

**Size:** M-L, ~3-4 days. Some heuristic design.

**Closes:** "Engineer A finished her queue while engineer B is
buried; cliptown should rebalance."

### Theme F1 — Local-LLM polish: ollama tested + opencode model spec

**Why:** Phase 3's local-LLM section (#55) documented the path but
nothing actually smoke-tested it. Operators following the guide
have no confidence cliptown works end-to-end against a real
ollama.

**Scope:**
- New `scripts/smoke-ollama.sh` — opt-in variant of
  `smoke-real-llm.sh` that boots an ollama-shaped fixture or
  pulls a small model (`llama3.1:8b`) and runs the haiku task
  through codex+ollama end-to-end.
- Verify the `OPENCODE_MODEL=ollama/...` prefix grammar actually
  works through opencode's adapter (#55 documents the convention
  but never tested it). Add a contract test against opencode's
  fixture CLI.
- Local-LLM Operator guide section in DEPLOY.md with a worked
  example + cost-savings table.

**Size:** S, ~1 day.

**Closes:** "Does cliptown actually run on ollama?"

### Theme F2 — Multi-cliptown federation (sketch)

**Why:** A power-user setup is "one cliptown per project, multiple
cliptowns talking to each other for cross-project coordination."
No primitive supports this today.

**Scope:** large; needs its own brainstorm before committing.
Probably: cliptown-to-cliptown WS link, shared skill registry, a
"foreign agent" handle for routing tasks across instances.

**Size:** XL, ~2-3 weeks.

**Closes:** "How do I run cliptown for two projects without
isolating the dev's brain into separate browser tabs?" Maybe
over-scoped for Phase 4; defer pending real demand.

### Theme G — Operator UX polish bucket

**Why:** Phase 3 shipped the wires; not every panel is loved.
SkillsPanel still no rollback UI (we deferred this in #72).
HistoryModal could use richer filtering. The operators panel could
group by role. Etc.

**Scope:** a rolling set of small TSX PRs. Ship as a Theme G with
no fixed size — drained opportunistically.

**Size:** XS-S each, total ~1 week if everything lands.

**Closes:** the "operator-console UI deferred" notes left by
#66 / #70 / #72.

## Recommended sequencing

1. **First**: Theme F1 (Local-LLM smoke) — small, high signal,
   validates Phase 3's marketing claim before we add anything.
2. **Second**: Theme E1 (Peer review) — most users will encounter
   this missing capability before the others. Modest scope.
3. **Third**: Theme E2 (Blocking dependencies) — natural follow-up
   to E1 because the cases interleave.
4. **Fourth**: Theme G (Operator UX polish) — fold in between
   E-theme work as palate cleansers.
5. **Fifth**: Theme E3 (Work-stealing) — biggest of the E themes,
   benefits from E1+E2 landing first.
6. **Sixth (separate brainstorm)**: Theme F2 (federation) — XL,
   defer pending real demand.

## What this doc is NOT

- Not a binding plan. Each theme should get its own brainstorm
  spec when scheduled.
- Not a deadline / commitment. Themes are sequenced for value, not
  time-boxed.
- Not exhaustive — Theme F2 acknowledges that there's a world of
  cross-cliptown ideas that need their own deep area.

## Suggested next concrete action

Pick **Theme F1** (Local-LLM smoke) for the first Phase 4 PR cycle.
It's small enough to land in a day, validates the local-LLM
narrative from #55, and de-risks the next E-themes by establishing
"local backends work end-to-end."

Open question to resolve at brainstorm time: do we ship F1 with
ollama only, or include LM Studio + vLLM in the same script via
the OpenAI-compat baseline?
