# Skills broadcasts + operator UI (read + attach/detach) — design

**Date:** 2026-05-13
**Status:** draft — pending implementation
**Driver:** P2.2 Phase 2 MVP shipped the skills DAO + MCP tools + worker integration but left two follow-ups in CHANGELOG known-limitations: no operator UI, no `skill_changed` ConsoleOutbound broadcasts. This spec partially closes both — read+attach/detach UI lands; create/edit/delete stays on MCP tools (heavier editor UX deferred to a separate PR).

## Goals

- `ConsoleOutbound::SkillChanged` broadcast on every mutation. Frontend hydrates a skills slice from snapshot + applies broadcasts.
- `ConsoleOutbound::SkillsSnapshot` delivered on console connect so the panel has data immediately.
- `ConsoleInbound::SkillAttach` / `SkillDetach` operator commands reuse `crate::skills::attach/detach` (operator-scoped, bypassing the agent-caller path of MCP `skill_attach`).
- Minimal **SkillsPanel** in the operator console: list current startup's skills with attached-agent badges; per-skill attach dropdown (unattached agents) + detach button.

## Non-goals (deferred to next polish PR)

- Skill content authoring UI. Operators still use MCP `skill_upsert` / `skill_delete` for now.
- Inline content viewer / editor.
- Cross-startup skills view (panel is scoped to currently-possessed startup).
- HTTP REST for operator-side mutations. Stay on WS.

## Wire format additions

### `ConsoleInbound` (2 new variants)

```rust
SkillAttach { v: u8, startup_id: String, agent_id: String, skill_id: String },
SkillDetach { v: u8, startup_id: String, agent_id: String, skill_id: String },
```

`startup_id` is explicit (operator scope can manage across startups).

### `ConsoleOutbound` (2 new variants)

```rust
SkillChanged {
    v: u8,
    startup_id: String,
    kind: String,             // "upsert" | "delete" | "attach" | "detach"
    skill_id: String,
    agent_id: Option<String>, // Some for attach/detach, None otherwise
},

SkillsSnapshot {
    v: u8,
    startups: Value,          // {sid: [{id, name, len, updated_at, attachments: [agent_id]}]}
},
```

## World-side wiring

`crates/world/src/skills.rs`:
- New `pub async fn list_with_attachments(pool, startup_id) → Result<Vec<SkillWithAttachments>>` returning `{id, name, len, updated_at, attachments: Vec<String>}`.
- New `pub async fn list_all_with_attachments(pool) → Result<HashMap<String, Vec<SkillWithAttachments>>>` keyed by `startup_id` — used to build SkillsSnapshot.

`crates/world/src/cmd_console.rs::dispatch`:
- 2 new arms: `SkillAttach` and `SkillDetach`. Each calls `crate::skills::attach/detach(pool, startup_id, agent_id, skill_id)` (operator caller — uses the passed `startup_id` as the authority since operators are workspace-wide). On success, broadcast `SkillChanged` via `event_tx`.

`crates/world/src/mcp_dispatch.rs`:
- 5 existing skill handlers (skill_upsert/list/attach/detach/delete) gain a `SkillChanged` broadcast after their successful mutation. The handlers already have `event_tx` access? Verify — if not, the dispatcher needs to pass it. Currently `handle_speak` does take `event_tx`; copy that pattern.

`crates/world/src/http.rs::ws_console`:
- After sending the initial WorldViewSnapshot, send `SkillsSnapshot` carrying `list_all_with_attachments` output.

## Frontend

`packages/frontend/src/store.ts`:
- Add `SkillVM { id, name, len, updated_at, attachments: string[] }`.
- `WorldState` gets `skills: Record<startupId, Record<skillId, SkillVM>>`.
- Reducers handle `skills_snapshot` and `skill_changed`. The `skill_changed` reducer:
  - `kind=upsert`: client doesn't have the full row from a broadcast alone. For this PR, mark stale and re-snapshot on next operator action. (Alternative: server includes `name/len/updated_at` in the SkillChanged broadcast. Simpler — let's do that.) → **Decision: broadcast carries `{name, len, updated_at, attachments}` so reducer applies in-place.**
  - `kind=delete`: drop from state.
  - `kind=attach`: append `agent_id` to that skill's attachments array.
  - `kind=detach`: remove `agent_id` from attachments.

**Revised `SkillChanged` shape:**

```rust
SkillChanged {
    v: u8,
    startup_id: String,
    kind: String,
    skill_id: String,
    agent_id: Option<String>,
    // For kind="upsert", echoes the row so frontend doesn't need a follow-up fetch.
    skill: Option<Value>,     // {id, name, len, updated_at, attachments}
},
```

`packages/frontend/src/console/SkillsPanel.tsx` *(new)*:
- Renders skill list for currently-possessed startup.
- Each skill row: name, len badge, list of attached agent badges (with × to detach), and an "Attach to..." dropdown of unattached agents in the startup.
- Stateless (reads from store; mutations emit WS via `ws.send`).
- Empty state: "No skills attached. Use `skill_upsert` MCP tool or SQL to create one."

`packages/frontend/src/console/Console.tsx`:
- Mount `<SkillsPanel />` as a card in the existing sidebar (or as a new tab — match existing layout idiom).

`packages/frontend/src/ws.ts`:
- Add `sendSkillAttach(...)`, `sendSkillDetach(...)`.

## Tests

**Rust unit (`crates/world/tests/cmd_console.rs` — new):**
- 2 tests: `skill_attach_via_console_emits_broadcast`, `skill_detach_via_console_emits_broadcast`.

**Rust integration (`crates/world/tests/skills_integration.rs` extension):**
- 1 test: `mcp_skill_upsert_emits_skill_changed_broadcast` — drive dispatch, assert event_rx receives the broadcast.

**Frontend Playwright (`packages/frontend/e2e/skills.spec.ts` — new):**
- 1 test: possess a startup with a pre-seeded skill → SkillsPanel renders → simulate skill_changed (kind=attach) frame → attachments badge appears.

**Smoke:** unchanged.

## Definition of done

- 2 new ConsoleInbound variants (SkillAttach, SkillDetach).
- 2 new ConsoleOutbound variants (SkillChanged, SkillsSnapshot).
- All 5 MCP skill handlers emit SkillChanged.
- SkillsPanel renders + attach/detach work end-to-end.
- Rust tests: 246 → 249 (+3).
- Frontend e2e: 14 → 15 (+1).
- CHANGELOG retires the two known-limitations bullets ("No UI" → reads "Read+attach/detach UI; create/edit/delete still via MCP" || "No skill_changed broadcasts" → fully retired).
