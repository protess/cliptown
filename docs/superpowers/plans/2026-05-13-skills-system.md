# P2.2 skills system (Phase 2 MVP) implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Workspace-scoped markdown skills attached many-to-many to agents; world exposes 5 MCP tools + 1 HTTP endpoint; worker fetches an agent's attached skills at `--real` spawn and writes them into the per-task execenv at `<workdir>/skills/<name>.md` with a CLAUDE.md listing.

**Architecture:** Two new SQL tables (`skills`, `agent_skills`) added via migration `0002_skills.sql`. Rust-side DAO in `crates/world/src/skills.rs` is a pure-SQLx module with 8 inline unit tests. MCP dispatcher gains 5 handlers; tool catalog grows from 16 to 21. New HTTP endpoint `GET /api/agents/:id/skills` returns the agent's attached skills with content for the worker to consume. Worker gets a thin `skills_client.ts` + `prepareWorkdir` extension.

**Tech Stack:** Rust (sqlx, axum, tokio, serde), TypeScript (Node fetch + node:fs/promises), vitest, SQLite.

**Spec:** `docs/superpowers/specs/2026-05-13-skills-system-design.md`

---

## File structure

- `crates/world/migrations/0002_skills.sql` *(new)* — `skills` + `agent_skills` tables with cascade FKs and unique-by-name-per-startup constraint.
- `crates/world/src/skills.rs` *(new)* — DAO: `upsert`, `list`, `attach`, `detach`, `delete`, `for_agent`, validators. Inline `#[cfg(test)]` unit tests.
- `crates/world/src/lib.rs` *(modify)* — `pub mod skills;` in alphabetical order.
- `crates/world/src/mcp_dispatch.rs` *(modify)* — 5 new handlers (`handle_skill_upsert`, `handle_skill_list`, `handle_skill_attach`, `handle_skill_detach`, `handle_skill_delete`) + 5 new arms in the dispatcher match.
- `crates/world/src/mcp_http.rs` *(modify)* — 5 new entries in `handle_tools_list()` JSON catalog.
- `crates/world/src/api_skills.rs` *(new)* — `GET /api/agents/:id/skills` handler with bearer auth check.
- `crates/world/src/http.rs` *(modify)* — route registration for `/api/agents/:agent_id/skills`.
- `crates/world/tests/skills_integration.rs` *(new)* — 3 integration tests booting `loop_::spawn` + dispatching MCP tools end to end.
- `crates/world/tests/api_skills.rs` *(new)* — 3 HTTP-level tests for the agent-skills endpoint.
- `packages/worker/src/skills_client.ts` *(new)* — `fetchSkillsForAgent(httpBase, agentId, secret) → Promise<SkillContent[]>`.
- `packages/worker/test/skills_client.test.ts` *(new)* — 2 tests over an in-process HTTP fixture.
- `packages/worker/src/execenv.ts` *(modify)* — `PrepareWorkdirOpts` gains optional `skills` array; `prepareWorkdir` writes them to `<workdir>/skills/<name>.md` and adds an "Available skills" section to CLAUDE.md.
- `packages/worker/test/execenv.test.ts` *(modify)* — 2 added tests covering the skills branch.
- `packages/worker/src/main.ts` *(modify)* — `--real` branch calls `fetchSkillsForAgent` before `prepareWorkdir`.
- `scripts/smoke-real-llm.sh` *(modify)* — pre-spawn step inserts a skill + attaches it; post-spawn block verifies the file lands.
- `CHANGELOG.md` + `TODOS.md` *(modify)* — M12 P2.2 section + Completed entry.

---

## Task 1: Schema migration + Rust DAO module

**Files:**
- Create: `crates/world/migrations/0002_skills.sql`
- Create: `crates/world/src/skills.rs`
- Modify: `crates/world/src/lib.rs`

- [ ] **Step 1: Write the migration**

Create `crates/world/migrations/0002_skills.sql` with:

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

- [ ] **Step 2: Write the DAO module with inline unit tests**

Create `crates/world/src/skills.rs` with:

```rust
//! P2.2 skills system: per-startup reusable markdown skills + many-to-many
//! attachment to agents. Pure SQLx module — no MCP / HTTP / async-task
//! concerns. The MCP dispatcher and the HTTP endpoint call into these
//! helpers; tests exercise them directly via TestCtx::pool.

use sqlx::SqlitePool;

/// 64 KB cap on a single skill's markdown. Defensive — agents that mention
/// "this skill" still need to read the whole thing into the prompt, and
/// pathological content shouldn't be able to blow up the model context.
pub const MAX_CONTENT_LEN: usize = 64 * 1024;

/// Skill name must fit `<workdir>/skills/<name>.md` cleanly across filesystems.
/// `[A-Za-z0-9_-]{1,64}` rejects path separators, dots, whitespace, unicode.
pub const MAX_NAME_LEN: usize = 64;

fn name_is_valid(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub id: String,
    pub startup_id: String,
    pub name: String,
    pub content_md: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillListing {
    pub id: String,
    pub name: String,
    pub updated_at: i64,
    pub len: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedSkill {
    pub name: String,
    pub content_md: String,
}

#[derive(Debug)]
pub enum SkillError {
    BadName,
    OversizeContent,
    NotFound,
    CrossStartup,
    Sql(String),
}

impl From<sqlx::Error> for SkillError {
    fn from(e: sqlx::Error) -> Self {
        SkillError::Sql(e.to_string())
    }
}

/// Insert a new skill or update an existing one (matched by `(startup_id, name)`).
/// Returns `(id, created)` — `created=true` for inserts, `false` for updates.
pub async fn upsert(
    pool: &SqlitePool,
    startup_id: &str,
    name: &str,
    content_md: &str,
) -> Result<(String, bool), SkillError> {
    if !name_is_valid(name) {
        return Err(SkillError::BadName);
    }
    if content_md.len() > MAX_CONTENT_LEN {
        return Err(SkillError::OversizeContent);
    }
    let existing: Option<(String,)> =
        sqlx::query_as("SELECT id FROM skills WHERE startup_id = ? AND name = ?")
            .bind(startup_id)
            .bind(name)
            .fetch_optional(pool)
            .await?;
    match existing {
        Some((id,)) => {
            sqlx::query(
                "UPDATE skills SET content_md = ?, updated_at = unixepoch() WHERE id = ?",
            )
            .bind(content_md)
            .bind(&id)
            .execute(pool)
            .await?;
            Ok((id, false))
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO skills (id, startup_id, name, content_md, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, unixepoch(), unixepoch())",
            )
            .bind(&id)
            .bind(startup_id)
            .bind(name)
            .bind(content_md)
            .execute(pool)
            .await?;
            Ok((id, true))
        }
    }
}

/// List skills in a startup. Excludes `content_md` for efficiency; callers
/// fetch the full row separately if they need the body.
pub async fn list(pool: &SqlitePool, startup_id: &str) -> Result<Vec<SkillListing>, SkillError> {
    let rows: Vec<(String, String, i64, i64)> = sqlx::query_as(
        "SELECT id, name, updated_at, length(content_md) FROM skills \
         WHERE startup_id = ? ORDER BY name",
    )
    .bind(startup_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, updated_at, len)| SkillListing { id, name, updated_at, len })
        .collect())
}

/// Fetch a single skill by id. Used by the MCP `skill_attach` path to
/// validate cross-startup constraints before the insert.
pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<Skill>, SkillError> {
    let row: Option<(String, String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT id, startup_id, name, content_md, created_at, updated_at \
         FROM skills WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, startup_id, name, content_md, created_at, updated_at)| Skill {
        id,
        startup_id,
        name,
        content_md,
        created_at,
        updated_at,
    }))
}

/// Idempotent attach. Verifies both the skill and the agent belong to
/// `caller_startup_id`. Already-attached is a no-op.
pub async fn attach(
    pool: &SqlitePool,
    caller_startup_id: &str,
    agent_id: &str,
    skill_id: &str,
) -> Result<(), SkillError> {
    let skill_row: Option<(String,)> =
        sqlx::query_as("SELECT startup_id FROM skills WHERE id = ?")
            .bind(skill_id)
            .fetch_optional(pool)
            .await?;
    let skill_startup = match skill_row {
        Some((s,)) => s,
        None => return Err(SkillError::NotFound),
    };
    if skill_startup != caller_startup_id {
        return Err(SkillError::CrossStartup);
    }
    let agent_row: Option<(String,)> =
        sqlx::query_as("SELECT startup_id FROM agents WHERE id = ?")
            .bind(agent_id)
            .fetch_optional(pool)
            .await?;
    let agent_startup = match agent_row {
        Some((s,)) => s,
        None => return Err(SkillError::NotFound),
    };
    if agent_startup != caller_startup_id {
        return Err(SkillError::CrossStartup);
    }
    sqlx::query(
        "INSERT INTO agent_skills (agent_id, skill_id, attached_at) \
         VALUES (?, ?, unixepoch()) \
         ON CONFLICT(agent_id, skill_id) DO NOTHING",
    )
    .bind(agent_id)
    .bind(skill_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Idempotent detach. Cross-startup checks mirror attach; not-attached is OK.
pub async fn detach(
    pool: &SqlitePool,
    caller_startup_id: &str,
    agent_id: &str,
    skill_id: &str,
) -> Result<(), SkillError> {
    // Same cross-startup guards (reuse via local fetches).
    let skill_row: Option<(String,)> =
        sqlx::query_as("SELECT startup_id FROM skills WHERE id = ?")
            .bind(skill_id)
            .fetch_optional(pool)
            .await?;
    let skill_startup = match skill_row {
        Some((s,)) => s,
        None => return Err(SkillError::NotFound),
    };
    if skill_startup != caller_startup_id {
        return Err(SkillError::CrossStartup);
    }
    let agent_row: Option<(String,)> =
        sqlx::query_as("SELECT startup_id FROM agents WHERE id = ?")
            .bind(agent_id)
            .fetch_optional(pool)
            .await?;
    let agent_startup = match agent_row {
        Some((s,)) => s,
        None => return Err(SkillError::NotFound),
    };
    if agent_startup != caller_startup_id {
        return Err(SkillError::CrossStartup);
    }
    sqlx::query("DELETE FROM agent_skills WHERE agent_id = ? AND skill_id = ?")
        .bind(agent_id)
        .bind(skill_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete a skill. Verifies caller owns it; cascading FK clears
/// `agent_skills` rows automatically.
pub async fn delete(
    pool: &SqlitePool,
    caller_startup_id: &str,
    skill_id: &str,
) -> Result<(), SkillError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT startup_id FROM skills WHERE id = ?")
            .bind(skill_id)
            .fetch_optional(pool)
            .await?;
    let owner = match row {
        Some((s,)) => s,
        None => return Err(SkillError::NotFound),
    };
    if owner != caller_startup_id {
        return Err(SkillError::CrossStartup);
    }
    sqlx::query("DELETE FROM skills WHERE id = ?")
        .bind(skill_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Fetch the agent's attached skills with full content. Used by the
/// HTTP endpoint that the worker calls at spawn time.
pub async fn for_agent(
    pool: &SqlitePool,
    agent_id: &str,
) -> Result<Vec<AttachedSkill>, SkillError> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT s.name, s.content_md FROM skills s \
         INNER JOIN agent_skills ags ON ags.skill_id = s.id \
         WHERE ags.agent_id = ? \
         ORDER BY s.name",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(name, content_md)| AttachedSkill { name, content_md })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage;

    async fn ctx() -> sqlx::SqlitePool {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        let pool = storage::open(p.to_str().unwrap()).await.unwrap();
        crate::seed::seed_if_empty(&pool).await.unwrap();
        // Insert a startup + 2 agents for cross-startup tests.
        sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, budget_spent_usd, created_at) VALUES ('S1','alpha','goal',10.0,0.0,unixepoch())")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, budget_spent_usd, created_at) VALUES ('S2','beta','goal',10.0,0.0,unixepoch())")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) VALUES ('A1','S1','eng','engineer','claude_code','m','{\"x\":0,\"y\":0}','lobby','idle')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) VALUES ('A2','S2','eng','engineer','claude_code','m','{\"x\":0,\"y\":0}','lobby','idle')")
            .execute(&pool).await.unwrap();
        // tempdir's drop runs at function-end if not held; we don't need it past pool open.
        std::mem::forget(dir);
        pool
    }

    #[tokio::test]
    async fn upsert_inserts_new_skill_with_id() {
        let pool = ctx().await;
        let (id, created) = upsert(&pool, "S1", "deploy-to-fly", "body").await.unwrap();
        assert!(created);
        assert!(!id.is_empty());
    }

    #[tokio::test]
    async fn upsert_updates_existing_skill_by_name() {
        let pool = ctx().await;
        let (id1, c1) = upsert(&pool, "S1", "deploy-to-fly", "v1").await.unwrap();
        let (id2, c2) = upsert(&pool, "S1", "deploy-to-fly", "v2").await.unwrap();
        assert!(c1);
        assert!(!c2);
        assert_eq!(id1, id2);
        let s = get(&pool, &id1).await.unwrap().unwrap();
        assert_eq!(s.content_md, "v2");
    }

    #[tokio::test]
    async fn upsert_rejects_bad_name_chars() {
        let pool = ctx().await;
        assert!(matches!(
            upsert(&pool, "S1", "bad name", "x").await,
            Err(SkillError::BadName)
        ));
        assert!(matches!(
            upsert(&pool, "S1", "..", "x").await,
            Err(SkillError::BadName)
        ));
        assert!(matches!(
            upsert(&pool, "S1", "", "x").await,
            Err(SkillError::BadName)
        ));
    }

    #[tokio::test]
    async fn upsert_rejects_oversize_content() {
        let pool = ctx().await;
        let big = "x".repeat(MAX_CONTENT_LEN + 1);
        assert!(matches!(
            upsert(&pool, "S1", "ok", &big).await,
            Err(SkillError::OversizeContent)
        ));
    }

    #[tokio::test]
    async fn attach_is_idempotent() {
        let pool = ctx().await;
        let (sid, _) = upsert(&pool, "S1", "deploy", "body").await.unwrap();
        attach(&pool, "S1", "A1", &sid).await.unwrap();
        attach(&pool, "S1", "A1", &sid).await.unwrap();
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT skill_id FROM agent_skills WHERE agent_id = ?")
                .bind("A1")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn attach_rejects_cross_startup_agent_or_skill() {
        let pool = ctx().await;
        let (sid, _) = upsert(&pool, "S1", "deploy", "body").await.unwrap();
        // Caller is S1 trying to attach S1's skill to S2's agent.
        assert!(matches!(
            attach(&pool, "S1", "A2", &sid).await,
            Err(SkillError::CrossStartup)
        ));
        // Caller is S2 trying to attach S1's skill (not theirs).
        assert!(matches!(
            attach(&pool, "S2", "A2", &sid).await,
            Err(SkillError::CrossStartup)
        ));
    }

    #[tokio::test]
    async fn delete_cascades_to_agent_skills() {
        let pool = ctx().await;
        let (sid, _) = upsert(&pool, "S1", "deploy", "body").await.unwrap();
        attach(&pool, "S1", "A1", &sid).await.unwrap();
        delete(&pool, "S1", &sid).await.unwrap();
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT skill_id FROM agent_skills WHERE agent_id = ?")
                .bind("A1")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn list_returns_metadata_only_no_content() {
        let pool = ctx().await;
        upsert(&pool, "S1", "deploy", "ten bytes!").await.unwrap();
        let items = list(&pool, "S1").await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "deploy");
        assert_eq!(items[0].len, 10);
    }

    #[tokio::test]
    async fn for_agent_returns_attached_with_content() {
        let pool = ctx().await;
        let (sid, _) = upsert(&pool, "S1", "deploy", "body").await.unwrap();
        attach(&pool, "S1", "A1", &sid).await.unwrap();
        let items = for_agent(&pool, "A1").await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "deploy");
        assert_eq!(items[0].content_md, "body");
    }
}
```

- [ ] **Step 3: Wire the module into the crate**

Open `crates/world/src/lib.rs`. Add `pub mod skills;` in alphabetical order with the other `pub mod` declarations.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p cliptown-world skills::tests 2>&1 | tail -10
```

Expected: 8 tests pass. Migration auto-applies because `storage::open` walks the migrations directory.

```bash
cargo test -p cliptown-world 2>&1 | grep "test result:" | awk '{sum += $4} END {print "rust:", sum}'
```

Expected: 231 + 8 = 239 (or 240 with the auto-generated ts-rs binding test if anything in this module derives `TS` — none should; this is server-internal).

- [ ] **Step 5: Commit**

```bash
git add crates/world/migrations/0002_skills.sql crates/world/src/skills.rs crates/world/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(world): skills + agent_skills tables + DAO

P2.2 Phase 2 MVP foundation. Migration 0002_skills.sql adds workspace-
scoped skills with a (startup_id, name) unique constraint, plus an
agent_skills join table. The DAO module owns upsert / list / attach /
detach / delete / for_agent helpers and validates skill names against
[A-Za-z0-9_-]{1,64} (filesystem-safe) and content_md ≤ 64 KB. 8 inline
unit tests cover the truth table including cross-startup rejection
and idempotent attach.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: MCP handlers + dispatcher wiring + tool catalog

**Files:**
- Modify: `crates/world/src/mcp_dispatch.rs`
- Modify: `crates/world/src/mcp_http.rs`
- Create: `crates/world/tests/skills_integration.rs`

- [ ] **Step 1: Write the failing integration tests**

Create `crates/world/tests/skills_integration.rs` with:

```rust
//! P2.2 MCP-level skills tests — round-trip through dispatch using a real
//! caller AvatarView. Skips the WS/HTTP outer layers and calls dispatch
//! directly with synthetic mcp_tool_use payloads.

use cliptown_world::loop_;
use cliptown_world::mcp_dispatch;
use cliptown_world::state::{AvatarView, WorldView};
use cliptown_world::storage;
use serde_json::json;
use sqlx::SqlitePool;
use std::collections::HashMap;
use tokio::sync::{broadcast, mpsc};

async fn ctx() -> (SqlitePool, AvatarView) {
    let dir = tempfile::tempdir().unwrap();
    let pool = storage::open(dir.path().join("t.db").to_str().unwrap())
        .await
        .unwrap();
    cliptown_world::seed::seed_if_empty(&pool).await.unwrap();
    sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, budget_spent_usd, created_at) VALUES ('S1','alpha','g',10.0,0.0,unixepoch())").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) VALUES ('A1','S1','eng','engineer','claude_code','m','{\"x\":0,\"y\":0}','lobby','idle')").execute(&pool).await.unwrap();
    std::mem::forget(dir);
    let caller = AvatarView {
        agent_id: "A1".to_string(),
        startup_id: "S1".to_string(),
        role: "engineer".to_string(),
        backend: "claude_code".to_string(),
        current_pos: (0, 0),
        target_pos: None,
        room_id: "lobby".to_string(),
        status: "idle".to_string(),
        last_seen_at: None,
        health: cliptown_world::health::Health::Offline,
    };
    (pool, caller)
}

async fn dispatch(
    pool: &SqlitePool,
    caller: &AvatarView,
    tool: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    let mut w = WorldView::default();
    w.avatars.insert(caller.agent_id.clone(), caller.clone());
    let mut paths = HashMap::new();
    let layout = cliptown_world::seed::TownLayout::default_town();
    let graph = cliptown_world::move_sys::graph_from_layout(&layout);
    let out_bus: HashMap<String, mpsc::Sender<serde_json::Value>> = HashMap::new();
    let (event_tx, _event_rx) = broadcast::channel(8);
    let msg = json!({
        "type": "mcp_tool_use",
        "v": 1,
        "corr_id": "c1",
        "tool": tool,
        "args": args,
    });
    mcp_dispatch::dispatch(
        &mut w, &mut paths, &layout, &graph, &out_bus, pool, &event_tx,
        &caller.agent_id, msg,
    )
    .await
}

#[tokio::test]
async fn mcp_skill_upsert_then_list_round_trip() {
    let (pool, caller) = ctx().await;
    let r = dispatch(
        &pool,
        &caller,
        "skill_upsert",
        json!({"name":"deploy","content_md":"hello"}),
    )
    .await;
    assert_eq!(r["type"], "mcp_reply");
    assert_eq!(r["result"]["created"], true);
    let id = r["result"]["id"].as_str().unwrap().to_string();
    assert!(!id.is_empty());

    let l = dispatch(&pool, &caller, "skill_list", json!({})).await;
    assert_eq!(l["type"], "mcp_reply");
    let items = l["result"]["skills"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "deploy");
    assert_eq!(items[0]["len"], 5);
}

#[tokio::test]
async fn mcp_skill_attach_then_detach() {
    let (pool, caller) = ctx().await;
    let u = dispatch(
        &pool,
        &caller,
        "skill_upsert",
        json!({"name":"deploy","content_md":"hello"}),
    )
    .await;
    let sid = u["result"]["id"].as_str().unwrap().to_string();
    let a = dispatch(
        &pool,
        &caller,
        "skill_attach",
        json!({"agent_id":"A1","skill_id":sid}),
    )
    .await;
    assert_eq!(a["type"], "mcp_reply");
    let count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM agent_skills WHERE agent_id = 'A1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 1);

    let d = dispatch(
        &pool,
        &caller,
        "skill_detach",
        json!({"agent_id":"A1","skill_id":sid}),
    )
    .await;
    assert_eq!(d["type"], "mcp_reply");
    let count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM agent_skills WHERE agent_id = 'A1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn mcp_skill_delete_cascades_attachment() {
    let (pool, caller) = ctx().await;
    let u = dispatch(
        &pool,
        &caller,
        "skill_upsert",
        json!({"name":"deploy","content_md":"hello"}),
    )
    .await;
    let sid = u["result"]["id"].as_str().unwrap().to_string();
    dispatch(
        &pool,
        &caller,
        "skill_attach",
        json!({"agent_id":"A1","skill_id":sid}),
    )
    .await;
    let d = dispatch(&pool, &caller, "skill_delete", json!({"skill_id":sid})).await;
    assert_eq!(d["type"], "mcp_reply");
    let count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM agent_skills WHERE agent_id = 'A1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 0);
}
```

- [ ] **Step 2: Verify red phase**

```bash
cargo test -p cliptown-world --test skills_integration 2>&1 | tail -15
```

Expected: FAIL — `unknown_tool` errors for all 5 skill_* tools because dispatcher doesn't handle them yet.

- [ ] **Step 3: Add the 5 handlers and dispatcher arms**

Open `crates/world/src/mcp_dispatch.rs`. Find the dispatcher match block (around line 97). Add 5 new arms BEFORE the `_ => Err(...)` default arm:

```rust
        "skill_upsert" => handle_skill_upsert(pool, &caller, args).await,
        "skill_list" => handle_skill_list(pool, &caller, args).await,
        "skill_attach" => handle_skill_attach(pool, &caller, args).await,
        "skill_detach" => handle_skill_detach(pool, &caller, args).await,
        "skill_delete" => handle_skill_delete(pool, &caller, args).await,
```

Then at the end of the file (after the existing handlers), append 5 new handler functions:

```rust
async fn handle_skill_upsert(
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let name = require_str(&args, "name")?.to_string();
    let content_md = require_str(&args, "content_md")?.to_string();
    match crate::skills::upsert(pool, &caller.startup_id, &name, &content_md).await {
        Ok((id, created)) => Ok(json!({"id": id, "created": created})),
        Err(crate::skills::SkillError::BadName) => Err((
            "bad_skill_name".into(),
            format!("name must match [A-Za-z0-9_-]{{1,64}}; got {name:?}"),
        )),
        Err(crate::skills::SkillError::OversizeContent) => Err((
            "skill_content_too_long".into(),
            format!("content_md exceeds {} bytes", crate::skills::MAX_CONTENT_LEN),
        )),
        Err(crate::skills::SkillError::NotFound) => {
            Err(("not_found".into(), "skill row vanished mid-upsert".into()))
        }
        Err(crate::skills::SkillError::CrossStartup) => {
            Err(("cross_startup".into(), "caller can't author skill in another startup".into()))
        }
        Err(crate::skills::SkillError::Sql(e)) => Err(("sql".into(), e)),
    }
}

async fn handle_skill_list(
    pool: &SqlitePool,
    caller: &AvatarView,
    _args: Value,
) -> HandlerResult {
    let items = crate::skills::list(pool, &caller.startup_id)
        .await
        .map_err(|e| match e {
            crate::skills::SkillError::Sql(s) => ("sql".to_string(), s),
            _ => ("sql".to_string(), "unexpected error".to_string()),
        })?;
    let arr: Vec<Value> = items
        .into_iter()
        .map(|s| {
            json!({
                "id": s.id,
                "name": s.name,
                "updated_at": s.updated_at,
                "len": s.len,
            })
        })
        .collect();
    Ok(json!({"skills": arr}))
}

async fn handle_skill_attach(
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let agent_id = require_str(&args, "agent_id")?.to_string();
    let skill_id = require_str(&args, "skill_id")?.to_string();
    match crate::skills::attach(pool, &caller.startup_id, &agent_id, &skill_id).await {
        Ok(()) => Ok(json!({"ok": true})),
        Err(crate::skills::SkillError::NotFound) => {
            Err(("not_found".into(), "agent or skill not found".into()))
        }
        Err(crate::skills::SkillError::CrossStartup) => Err((
            "cross_startup".into(),
            "agent or skill belongs to another startup".into(),
        )),
        Err(crate::skills::SkillError::Sql(e)) => Err(("sql".into(), e)),
        Err(_) => Err(("sql".into(), "unexpected error".into())),
    }
}

async fn handle_skill_detach(
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let agent_id = require_str(&args, "agent_id")?.to_string();
    let skill_id = require_str(&args, "skill_id")?.to_string();
    match crate::skills::detach(pool, &caller.startup_id, &agent_id, &skill_id).await {
        Ok(()) => Ok(json!({"ok": true})),
        Err(crate::skills::SkillError::NotFound) => {
            Err(("not_found".into(), "agent or skill not found".into()))
        }
        Err(crate::skills::SkillError::CrossStartup) => Err((
            "cross_startup".into(),
            "agent or skill belongs to another startup".into(),
        )),
        Err(crate::skills::SkillError::Sql(e)) => Err(("sql".into(), e)),
        Err(_) => Err(("sql".into(), "unexpected error".into())),
    }
}

async fn handle_skill_delete(
    pool: &SqlitePool,
    caller: &AvatarView,
    args: Value,
) -> HandlerResult {
    let skill_id = require_str(&args, "skill_id")?.to_string();
    match crate::skills::delete(pool, &caller.startup_id, &skill_id).await {
        Ok(()) => Ok(json!({"ok": true})),
        Err(crate::skills::SkillError::NotFound) => {
            Err(("not_found".into(), format!("no skill: {skill_id}")))
        }
        Err(crate::skills::SkillError::CrossStartup) => Err((
            "cross_startup".into(),
            "skill belongs to another startup".into(),
        )),
        Err(crate::skills::SkillError::Sql(e)) => Err(("sql".into(), e)),
        Err(_) => Err(("sql".into(), "unexpected error".into())),
    }
}
```

- [ ] **Step 4: Add the 5 tool catalog entries**

Open `crates/world/src/mcp_http.rs`. Find `handle_tools_list()` (around line 185). At the end of the `tools` JSON array (before the closing `]`), add 5 new entries. The existing pattern uses a `tool(name, desc, schema)` helper. Add:

```rust
        tool(
            "skill_upsert",
            "Author or update a workspace-scoped markdown skill.",
            json!({
                "type": "object",
                "properties": {
                    "name":       {"type": "string"},
                    "content_md": {"type": "string"}
                },
                "required": ["name", "content_md"]
            }),
        ),
        tool(
            "skill_list",
            "List skills in the caller's startup (metadata only, no content).",
            json!({"type": "object", "properties": {}}),
        ),
        tool(
            "skill_attach",
            "Attach a skill to an agent in the caller's startup.",
            json!({
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "skill_id": {"type": "string"}
                },
                "required": ["agent_id", "skill_id"]
            }),
        ),
        tool(
            "skill_detach",
            "Detach a skill from an agent. Idempotent.",
            json!({
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "skill_id": {"type": "string"}
                },
                "required": ["agent_id", "skill_id"]
            }),
        ),
        tool(
            "skill_delete",
            "Delete a skill. Cascades to all attachments.",
            json!({
                "type": "object",
                "properties": {
                    "skill_id": {"type": "string"}
                },
                "required": ["skill_id"]
            }),
        ),
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p cliptown-world --test skills_integration 2>&1 | tail -10
```

Expected: 3 tests pass.

```bash
cargo test -p cliptown-world 2>&1 | grep "test result:" | awk '{sum += $4} END {print "rust:", sum}'
```

Expected: 231 + 8 (Task 1 unit) + 3 (Task 2 integration) = 242.

- [ ] **Step 6: Commit**

```bash
git add crates/world/src/mcp_dispatch.rs crates/world/src/mcp_http.rs crates/world/tests/skills_integration.rs
git commit -m "$(cat <<'EOF'
feat(world): 5 skill_* MCP tools + dispatcher + catalog

skill_upsert / skill_list / skill_attach / skill_detach / skill_delete
land in mcp_dispatch with consistent cross-startup checks. The tools/
list HTTP catalog grows from 16 to 21 entries. 3 integration tests
exercise the round-trip end to end via mcp_dispatch::dispatch.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: HTTP API endpoint `GET /api/agents/:id/skills`

**Files:**
- Create: `crates/world/src/api_skills.rs`
- Modify: `crates/world/src/lib.rs`
- Modify: `crates/world/src/http.rs`
- Create: `crates/world/tests/api_skills.rs`

- [ ] **Step 1: Write the HTTP-level integration tests (red)**

Create `crates/world/tests/api_skills.rs` with:

```rust
//! P2.2 HTTP endpoint tests — boot the axum router + send real requests.
//! Verifies bearer auth + JSON body shape that the worker depends on.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cliptown_world::http::{router, AppState};
use cliptown_world::loop_;
use cliptown_world::state::WorldView;
use cliptown_world::storage;
use http_body_util::BodyExt;
use std::sync::Arc;
use tokio::sync::broadcast;
use tower::ServiceExt;

async fn fixture() -> AppState {
    let dir = tempfile::tempdir().unwrap();
    let pool = storage::open(dir.path().join("t.db").to_str().unwrap())
        .await
        .unwrap();
    cliptown_world::seed::seed_if_empty(&pool).await.unwrap();
    sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, budget_spent_usd, created_at) VALUES ('S1','alpha','g',10.0,0.0,unixepoch())").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) VALUES ('A1','S1','eng','engineer','claude_code','m','{\"x\":0,\"y\":0}','lobby','idle')").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO agent_secrets (agent_id, secret) VALUES ('A1','sec1')")
        .execute(&pool)
        .await
        .unwrap();
    let (sid, _) = cliptown_world::skills::upsert(&pool, "S1", "deploy", "hello")
        .await
        .unwrap();
    cliptown_world::skills::attach(&pool, "S1", "A1", &sid)
        .await
        .unwrap();
    std::mem::forget(dir);
    let (event_tx, _event_rx) = broadcast::channel(8);
    let supervisor = Arc::new(cliptown_world::agent_supervisor::AgentSupervisor::new(
        cliptown_world::agent_supervisor::SupervisorConfig {
            worker_bin: "/bin/sh".into(),
            worker_args: vec![],
            backoff_ms: vec![],
            dissolve_grace_ms: 100,
        },
        pool.clone(),
        event_tx.clone(),
    ));
    let handle = loop_::spawn(WorldView::default(), pool.clone(), event_tx);
    AppState {
        pool,
        handle,
        catalog: Arc::new(tokio::sync::RwLock::new(Default::default())),
        supervisor,
        max_review_rounds: 3,
    }
}

#[tokio::test]
async fn get_agent_skills_returns_attached_with_content() {
    let state = fixture().await;
    let app = router(state);
    let req = Request::builder()
        .method("GET")
        .uri("/api/agents/A1/skills")
        .header("Authorization", "Bearer A1:sec1")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = v["skills"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "deploy");
    assert_eq!(items[0]["content_md"], "hello");
}

#[tokio::test]
async fn get_agent_skills_rejects_wrong_bearer() {
    let state = fixture().await;
    let app = router(state);
    let req = Request::builder()
        .method("GET")
        .uri("/api/agents/A1/skills")
        .header("Authorization", "Bearer A1:wrong")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_agent_skills_rejects_mismatched_path_id() {
    let state = fixture().await;
    let app = router(state);
    // Agent A1's token used to fetch A2's skills (even if A2 doesn't exist).
    let req = Request::builder()
        .method("GET")
        .uri("/api/agents/A2/skills")
        .header("Authorization", "Bearer A1:sec1")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
```

If the test references `agent_secrets` or `cliptown_world::auth` shapes that don't exist as expected (the auth module may use a different secret table), open `crates/world/src/auth.rs` and adjust the fixture's `INSERT` line to use the actual schema. The bearer-validation function exists; use whichever helper validates `<agent_id>:<secret>` (the existing MCP HTTP uses it).

- [ ] **Step 2: Verify it fails red**

```bash
cargo test -p cliptown-world --test api_skills 2>&1 | tail -15
```

Expected: FAIL — route `/api/agents/:agent_id/skills` returns 404 because it's not registered yet (or, if registration is the issue, the handler is missing).

- [ ] **Step 3: Inspect the existing bearer auth pattern**

Open `crates/world/src/mcp_http.rs` and find the `authenticate` function. Note the signature — typically `authenticate(pool, headers) → Result<AvatarView, _>` or similar. The new endpoint reuses this. If it takes `headers` and returns an `AvatarView`, use it directly.

Run:

```bash
grep -n "fn authenticate\|fn validate_agent_token" crates/world/src/mcp_http.rs crates/world/src/auth.rs
```

Note the existing helper signature. Use it in Step 4.

- [ ] **Step 4: Create the HTTP handler**

Create `crates/world/src/api_skills.rs` with:

```rust
//! P2.2 HTTP endpoint for the worker to fetch an agent's attached skills.
//! Bearer auth via `<agent_id>:<secret>` (same scheme as MCP HTTP). The
//! caller must equal `:agent_id` in the path.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

use crate::http::AppState;

pub async fn get_agent_skills(
    Path(agent_id): Path<String>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    // Bearer is "Authorization: Bearer <agent_id>:<secret>".
    let header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let (claimed_agent, secret) = match header.split_once(':') {
        Some((a, s)) => (a, s),
        None => {
            return (StatusCode::UNAUTHORIZED, Json(json!({"error": "bad_auth_format"})))
                .into_response();
        }
    };
    // The claimed agent must match the path agent.
    if claimed_agent != agent_id {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "agent_id mismatch"})),
        )
            .into_response();
    }
    // Validate <agent_id>:<secret> against the agents table.
    let row: Option<(String,)> =
        sqlx::query_as("SELECT secret FROM agent_secrets WHERE agent_id = ?")
            .bind(&agent_id)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);
    let stored_secret = match row {
        Some((s,)) => s,
        None => {
            return (StatusCode::UNAUTHORIZED, Json(json!({"error": "no_secret"})))
                .into_response();
        }
    };
    if stored_secret != secret {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "bad_secret"})))
            .into_response();
    }
    // Fetch attached skills.
    let items = match crate::skills::for_agent(&state.pool, &agent_id).await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("{e:?}")})),
            )
                .into_response();
        }
    };
    let json_items: Vec<serde_json::Value> = items
        .into_iter()
        .map(|s| json!({"name": s.name, "content_md": s.content_md}))
        .collect();
    (StatusCode::OK, Json(json!({"skills": json_items}))).into_response()
}
```

If the secret storage is in a different table than `agent_secrets`, replace the SQL accordingly. (Search `grep -rn "secret" crates/world/src/auth.rs crates/world/src/mcp_http.rs | head -10` to confirm the column/table name.) The fixture in the test references the same schema; both must match the actual production code.

- [ ] **Step 5: Wire the module + route**

Open `crates/world/src/lib.rs`. Add `pub mod api_skills;` in alphabetical order.

Open `crates/world/src/http.rs`. Find the `router` function (around line 25). Add a new route between the existing routes:

```rust
        .route(
            "/api/agents/:agent_id/skills",
            get(crate::api_skills::get_agent_skills),
        )
```

(Place it near other `/api/` routes for grouping.) If `get` isn't already imported, add it to the imports at the top of `http.rs`.

- [ ] **Step 6: Run tests**

```bash
cargo test -p cliptown-world --test api_skills 2>&1 | tail -10
```

Expected: 3 tests pass.

```bash
cargo test -p cliptown-world 2>&1 | grep "test result:" | awk '{sum += $4} END {print "rust:", sum}'
```

Expected: 231 + 8 + 3 + 3 = 245.

- [ ] **Step 7: Commit**

```bash
git add crates/world/src/api_skills.rs crates/world/src/lib.rs crates/world/src/http.rs crates/world/tests/api_skills.rs
git commit -m "$(cat <<'EOF'
feat(world): GET /api/agents/:id/skills endpoint

Returns the agent's attached skills with full content_md for the
worker to write into <workdir>/skills/<name>.md at spawn time. Bearer
auth via <agent_id>:<secret> matches the MCP HTTP scheme; the bearer's
agent must equal the path's agent. 3 HTTP-level integration tests
cover happy path, wrong-secret, and cross-agent rejection.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Worker `skills_client.ts` + unit tests

**Files:**
- Create: `packages/worker/src/skills_client.ts`
- Create: `packages/worker/test/skills_client.test.ts`

- [ ] **Step 1: Write the failing test file**

Create `packages/worker/test/skills_client.test.ts` with:

```ts
import { describe, it, expect } from "vitest";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { fetchSkillsForAgent } from "../src/skills_client.js";

interface Handle {
  url: string;
  close: () => Promise<void>;
}

async function startServer(
  handler: (req: import("node:http").IncomingMessage, res: import("node:http").ServerResponse) => void,
): Promise<Handle> {
  const server: Server = createServer(handler);
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", () => r()));
  const port = (server.address() as AddressInfo).port;
  return {
    url: `http://127.0.0.1:${port}`,
    async close() {
      await new Promise<void>((r) => server.close(() => r()));
    },
  };
}

describe("fetchSkillsForAgent", () => {
  it("returns parsed list on 200", async () => {
    const h = await startServer((req, res) => {
      if (req.method === "GET" && req.url === "/api/agents/A1/skills" &&
          req.headers.authorization === "Bearer A1:sec1") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ skills: [{ name: "deploy", content_md: "body" }] }));
        return;
      }
      res.writeHead(500); res.end();
    });
    const skills = await fetchSkillsForAgent(h.url, "A1", "sec1");
    expect(skills).toEqual([{ name: "deploy", content_md: "body" }]);
    await h.close();
  });

  it("returns [] when the agent has no attached skills (200 + empty array)", async () => {
    const h = await startServer((req, res) => {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ skills: [] }));
    });
    const skills = await fetchSkillsForAgent(h.url, "A1", "sec1");
    expect(skills).toEqual([]);
    await h.close();
  });

  it("throws on non-2xx", async () => {
    const h = await startServer((req, res) => {
      res.writeHead(401, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: "unauthorized" }));
    });
    await expect(fetchSkillsForAgent(h.url, "A1", "sec1")).rejects.toThrow(/status=401/);
    await h.close();
  });
});
```

- [ ] **Step 2: Verify red**

```bash
pnpm -F @cliptown/worker test -- --run skills_client.test.ts 2>&1 | tail -10
```

Expected: FAIL — module `../src/skills_client.js` not found.

- [ ] **Step 3: Implement the module**

Create `packages/worker/src/skills_client.ts` with:

```ts
/**
 * P2.2 worker-side skills fetcher. Single GET against the world's
 * /api/agents/:id/skills endpoint with bearer auth. The shape is the
 * one prepareWorkdir consumes — { name, content_md }[] — verbatim.
 */

export interface SkillContent {
  name: string;
  content_md: string;
}

export async function fetchSkillsForAgent(
  worldHttpBase: string,
  agentId: string,
  secret: string,
): Promise<SkillContent[]> {
  const url = `${worldHttpBase.replace(/\/$/, "")}/api/agents/${encodeURIComponent(agentId)}/skills`;
  const res = await fetch(url, {
    method: "GET",
    headers: { Authorization: `Bearer ${agentId}:${secret}` },
  });
  if (!res.ok) {
    throw new Error(`fetchSkillsForAgent failed: status=${res.status}`);
  }
  const body = (await res.json()) as { skills?: SkillContent[] };
  return Array.isArray(body.skills) ? body.skills : [];
}
```

- [ ] **Step 4: Run tests**

```bash
pnpm -F @cliptown/worker test -- --run skills_client.test.ts 2>&1 | tail -5
```

Expected: 3 tests pass.

```bash
pnpm -F @cliptown/worker test 2>&1 | tail -5
```

Expected: 70 (existing) + 3 (skills_client) = 73 total.

- [ ] **Step 5: Commit**

```bash
git add packages/worker/src/skills_client.ts packages/worker/test/skills_client.test.ts
git commit -m "$(cat <<'EOF'
feat(worker): skills_client for /api/agents/:id/skills

Thin fetch wrapper around the world's HTTP endpoint. Returns the
shape prepareWorkdir consumes verbatim. 3 unit tests over an
in-process http server: happy path, empty list, non-2xx error.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Extend `prepareWorkdir` + wire into `main.ts`

**Files:**
- Modify: `packages/worker/src/execenv.ts`
- Modify: `packages/worker/test/execenv.test.ts`
- Modify: `packages/worker/src/main.ts`

- [ ] **Step 1: Add the 2 new failing tests**

Open `packages/worker/test/execenv.test.ts`. After the existing tests (still inside the `describe("prepareWorkdir", ...)` block), add:

```ts
  it("writes attached skills as <workdir>/skills/<name>.md and lists them in CLAUDE.md", async () => {
    const workdir = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
      skills: [
        { name: "deploy-to-fly", content_md: "deploy steps" },
        { name: "read-logs", content_md: "log locations" },
      ],
    });
    const deploy = await readFile(join(workdir, "skills", "deploy-to-fly.md"), "utf-8");
    const logs = await readFile(join(workdir, "skills", "read-logs.md"), "utf-8");
    expect(deploy).toBe("deploy steps");
    expect(logs).toBe("log locations");
    const claudeMd = await readFile(join(workdir, "CLAUDE.md"), "utf-8");
    expect(claudeMd).toContain("## Available skills");
    expect(claudeMd).toContain("deploy-to-fly");
    expect(claudeMd).toContain("./skills/deploy-to-fly.md");
    expect(claudeMd).toContain("read-logs");
  });

  it("omits skills section and skills dir when skills is empty or absent", async () => {
    const workdir = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
      skills: [],
    });
    const claudeMd = await readFile(join(workdir, "CLAUDE.md"), "utf-8");
    expect(claudeMd).not.toContain("## Available skills");
    // skills/ directory should not exist
    await expect(stat(join(workdir, "skills"))).rejects.toThrow();
  });
```

- [ ] **Step 2: Verify red**

```bash
pnpm -F @cliptown/worker test -- --run execenv.test.ts 2>&1 | tail -10
```

Expected: 2 new tests FAIL because `prepareWorkdir` doesn't write skills yet.

- [ ] **Step 3: Extend `prepareWorkdir`**

Open `packages/worker/src/execenv.ts`. Add a `SkillContent` interface (exported) and an optional `skills` field on `PrepareWorkdirOpts`:

```ts
export interface SkillContent {
  name: string;
  content_md: string;
}

export interface PrepareWorkdirOpts {
  /** Absolute path passed via --workspace; the parent of the workspaces tree. */
  workspacesRoot: string;
  startupId: string;
  taskId: string;
  agentId: string;
  /** Optional: per-agent attached skills to materialize at <workdir>/skills/<name>.md. */
  skills?: SkillContent[];
}
```

In the `prepareWorkdir` body, after writing CLAUDE.md, add the skills write step. The full updated function:

```ts
export async function prepareWorkdir(opts: PrepareWorkdirOpts): Promise<string> {
  const wsRoot = resolve(opts.workspacesRoot);
  const workspacesDir = join(wsRoot, "workspaces");
  const workdir = join(workspacesDir, opts.startupId, opts.taskId, "workdir");

  await mkdir(workdir, { recursive: true });
  await mkdir(workspacesDir, { recursive: true });

  const linkPath = join(workdir, "workspaces");
  try {
    await symlink(workspacesDir, linkPath);
  } catch (e) {
    const err = e as NodeJS.ErrnoException;
    if (err.code !== "EEXIST") throw err;
    let existing: string | null = null;
    try {
      existing = await readlink(linkPath);
    } catch {
      existing = null;
    }
    if (existing !== workspacesDir) {
      throw new Error(
        `workdir/workspaces exists but doesn't point to ${workspacesDir} (got ${existing ?? "non-link entry"})`,
      );
    }
  }

  const skills = opts.skills ?? [];
  if (skills.length > 0) {
    const skillsDir = join(workdir, "skills");
    await mkdir(skillsDir, { recursive: true });
    for (const s of skills) {
      await writeFile(join(skillsDir, `${s.name}.md`), s.content_md, "utf-8");
    }
  }

  const claudeMd = buildClaudeMd(opts, skills);
  await writeFile(join(workdir, "CLAUDE.md"), claudeMd, "utf-8");

  return workdir;
}
```

Update `buildClaudeMd` to accept skills and emit the section:

```ts
function buildClaudeMd(opts: PrepareWorkdirOpts, skills: SkillContent[]): string {
  const { agentId, taskId, startupId } = opts;
  const canonical = `workspaces/${startupId}/artifacts/${taskId}.md`;
  const lines = [
    "# Task context",
    "",
    `You are agent \`${agentId}\` running task \`${taskId}\` for startup \`${startupId}\`.`,
    "",
    "## Working directory layout",
    "",
    "- `./workspaces/` — symlink to the shared workspaces tree. The canonical artifact path for this task is `" +
      canonical +
      "` (relative to this workdir).",
    "- Anything else you create directly in this workdir is per-task scratch and survives the session until GC.",
    "",
    "## When you're done",
    "",
    "Call the MCP tool `task_done` with `task_id = \"" +
      taskId +
      "\"` and `artifact_path = \"" +
      canonical +
      "\"`. The world enforces this exact path.",
    "",
  ];
  if (skills.length > 0) {
    lines.push("## Available skills");
    lines.push("");
    lines.push("You have these reusable skills attached. Read them when relevant:");
    lines.push("");
    for (const s of skills) {
      lines.push(`- \`${s.name}\` → \`./skills/${s.name}.md\``);
    }
    lines.push("");
  }
  return lines.join("\n");
}
```

Also add the `stat` import to the test file (it's used in the second new test). Check `packages/worker/test/execenv.test.ts` imports — `stat` was already imported at the top. If not, add it.

- [ ] **Step 4: Wire into main.ts**

Open `packages/worker/src/main.ts`. Add the import at the top:

```ts
import { fetchSkillsForAgent } from "./skills_client.js";
```

In the `--real` branch, BEFORE the existing `prepareWorkdir` call, fetch skills:

```ts
    // P2.2: fetch attached skills from world. The prepareWorkdir call below
    // will write each as <workdir>/skills/<name>.md and reference them in
    // CLAUDE.md. Failure here is logged but doesn't block — an agent with
    // no skills (or a world that doesn't support /api/agents/:id/skills)
    // proceeds with an empty list.
    let skills: SkillContent[] = [];
    try {
      skills = await fetchSkillsForAgent(mcpWorldUrl, args.agentId, args.secret);
      if (skills.length > 0) {
        console.log(`[worker] fetched ${skills.length} skill(s): ${skills.map((s) => s.name).join(", ")}`);
      }
    } catch (e) {
      console.warn(`[worker] fetchSkillsForAgent failed (continuing without skills): ${(e as Error).message}`);
    }
```

Add the `SkillContent` import alongside `fetchSkillsForAgent`:

```ts
import { fetchSkillsForAgent, type SkillContent } from "./skills_client.js";
```

Then update the existing `prepareWorkdir` call to pass skills:

```ts
    const workdir = await prepareWorkdir({
      workspacesRoot: workspaceRoot,
      startupId: args.startupId,
      taskId: args.taskId,
      agentId: args.agentId,
      skills,
    });
```

- [ ] **Step 5: Run all worker tests**

```bash
pnpm -F @cliptown/worker test 2>&1 | tail -5
```

Expected: 70 (existing) + 3 (skills_client) + 2 (execenv extension) = 75 total.

- [ ] **Step 6: Commit**

```bash
git add packages/worker/src/execenv.ts packages/worker/test/execenv.test.ts packages/worker/src/main.ts
git commit -m "$(cat <<'EOF'
feat(worker): write attached skills into per-task execenv

prepareWorkdir gains an optional skills array; when non-empty it
mkdir's <workdir>/skills/ and writes each skill as <name>.md, and
CLAUDE.md gains an "Available skills" section listing each name +
relative path. The --real branch in main.ts calls fetchSkillsForAgent
before prepareWorkdir; fetch failures log a warning and proceed with
an empty list.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Smoke script — skill seed + disk verification

**Files:**
- Modify: `scripts/smoke-real-llm.sh`

- [ ] **Step 1: Add skill seeding before worker spawn**

Open `scripts/smoke-real-llm.sh`. Find the section that seeds the parent task (search for `seeding parent task`). After that block (and after the engineer agent id is known), add a new section:

```bash
# ── 5.5. seed a skill + attach to engineer (P2.2 verification) ─────────────
say "seeding skill 'smoke-skill-deploy' and attaching to engineer"
SKILL_ID="$(uuidgen | tr 'A-Z' 'a-z')"
SKILL_CONTENT="Smoke test skill content. The agent should see this file in its execenv."
sqlite3 "$SMOKE_DIR/cliptown.db" <<SQL
INSERT INTO skills (id, startup_id, name, content_md, created_at, updated_at)
  VALUES ('$SKILL_ID', '$STARTUP_ID', 'smoke-skill-deploy', '$SKILL_CONTENT', unixepoch(), unixepoch());
INSERT INTO agent_skills (agent_id, skill_id, attached_at)
  VALUES ('$ENGINEER_ID', '$SKILL_ID', unixepoch());
SQL
```

(If `uuidgen` isn't available on the test host, use `cat /proc/sys/kernel/random/uuid` on Linux or fallback `python3 -c "import uuid; print(uuid.uuid4())"`. The smoke script's existing patterns may show a precedent — match whatever they do.)

- [ ] **Step 2: Add the post-spawn verification**

Find the `# ── 7.5. verify: per-task execenv` block (added in P2.3). After that section, add:

```bash
# ── 7.6. verify: skill landed in execenv (P2.2) ────────────────────────────
say "verify: attached skill at workspaces/$STARTUP_ID/$TASK_ID/workdir/skills/"
SKILL_FILE="$EXECENV_WORKDIR/skills/smoke-skill-deploy.md"
[[ -f "$SKILL_FILE" ]] || fail "skill file missing: $SKILL_FILE"
grep -q "Smoke test skill content" "$SKILL_FILE" \
  || fail "skill file content mismatch"
grep -q "smoke-skill-deploy" "$EXECENV_WORKDIR/CLAUDE.md" \
  || fail "CLAUDE.md does not mention attached skill 'smoke-skill-deploy'"
say "skill check passed: skill file + CLAUDE.md reference both present"
```

- [ ] **Step 3: Validate syntax (no real run)**

```bash
bash -n scripts/smoke-real-llm.sh && echo "OK: script syntax clean"
```

Expected: `OK: script syntax clean`.

- [ ] **Step 4: Commit**

```bash
git add scripts/smoke-real-llm.sh
git commit -m "$(cat <<'EOF'
chore(smoke): seed a skill + verify it lands in execenv

Pre-spawn step inserts a skill 'smoke-skill-deploy' into SQL and
attaches it to the engineer agent. Post-spawn verification asserts:
- <workdir>/skills/smoke-skill-deploy.md exists with expected content.
- <workdir>/CLAUDE.md mentions the skill name in the "Available
  skills" section.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: CHANGELOG + TODOS + verification sweep

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `TODOS.md`

- [ ] **Step 1: Insert the M12 P2.2 CHANGELOG section atop**

Find `## M12 — P2.3 per-task execenv directories (2026-05-13)` near the top. Insert ABOVE it:

```markdown
## M12 — P2.2 skills system (Phase 2 MVP, 2026-05-13)

Per-startup reusable markdown skills attached many-to-many to agents.
At `--real` adapter spawn the worker fetches the agent's attached
skills and writes each as `<workdir>/skills/<name>.md` (alongside
CLAUDE.md and the workspaces symlink from P2.3). CLAUDE.md gains an
"Available skills" section listing each skill's name and relative
path.

- **Schema:** `skills` (workspace-scoped, `UNIQUE(startup_id, name)`)
  + `agent_skills` (M:N attachment). Migration `0002_skills.sql`.
- **DAO:** `crates/world/src/skills.rs` with 8 inline unit tests.
  Names constrained to `[A-Za-z0-9_-]{1,64}` (filesystem-safe);
  content capped at 64 KB.
- **MCP tools (5 new, catalog 16 → 21):** `skill_upsert`,
  `skill_list`, `skill_attach`, `skill_detach`, `skill_delete`.
  All enforce cross-startup checks.
- **HTTP API:** `GET /api/agents/:agent_id/skills` returns
  `{skills: [{name, content_md}]}` for the worker. Bearer auth via
  `<agent_id>:<secret>` matches MCP HTTP.
- **Worker:** new `skills_client.ts::fetchSkillsForAgent` +
  `prepareWorkdir` extension write skills into the execenv.
- **Smoke:** seeds `smoke-skill-deploy` + attaches it + asserts the
  file lands in the per-task workdir.

### Known limitations carried forward

- No frontend skill management UI. Operators manage skills via MCP
  tools or direct SQL.
- No `skill_changed` ConsoleOutbound broadcasts. Lazy fetch at spawn
  is the contract; live edits don't affect in-flight tasks.
- No global (non-workspace) skills.
- No file attachments beyond the markdown content_md body.
- No versioning / history (upsert is mutable; latest wins).

```

- [ ] **Step 2: Add the TODOS entry**

Open `TODOS.md`. Under `## Completed`, ABOVE the existing `### M12 P2.3 per-task execenv directories` entry, insert:

```markdown
### M12 P2.2 skills system — 2026-05-13
**Source:** Phase 2 backlog last item (from `docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md` § P2.2). PR `<TBD — fill in at PR creation>`.

Was: cliptown agents saw only `task.title` + `task.description`. No way to compound reusable capability — every new agent session reinvented the wheel.

Fixed: per-startup markdown skills attached many-to-many to agents. SQL: `skills` + `agent_skills` tables (migration `0002_skills.sql`). World: `crates/world/src/skills.rs` DAO + 5 MCP tools (`skill_upsert` / `skill_list` / `skill_attach` / `skill_detach` / `skill_delete`) + HTTP endpoint `GET /api/agents/:id/skills`. Worker: `skills_client.ts::fetchSkillsForAgent` + `prepareWorkdir` extension writes each attached skill as `<workdir>/skills/<name>.md` and adds an "Available skills" section to CLAUDE.md. Smoke seeds a skill + verifies on-disk landing. Frontend UI / `skill_changed` broadcasts / global skills / file attachments / versioning all deferred (Known limitations).

```

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md TODOS.md
git commit -m "$(cat <<'EOF'
docs: M12 P2.2 skills system changelog + TODOS

CHANGELOG gains the M12 P2.2 section atop (Phase 2 MVP). TODOS
Completed gets the matching entry with TBD PR placeholder (filled
at PR creation). Known limitations enumerate UI / broadcasts / global
skills / file attachments / versioning as deferred follow-ups.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 4: Full verification sweep**

Run and report:

```bash
cargo test -p cliptown-world 2>&1 | grep "test result:" | awk '{sum += $4} END {print "rust:", sum}'
pnpm -F @cliptown/adapter-core test 2>&1 | grep -E "Tests *[0-9]+ passed" | head -1
pnpm -F @cliptown/adapter-claude-code test 2>&1 | grep -E "Tests *[0-9]+ passed" | head -1
pnpm -F @cliptown/adapter-codex test 2>&1 | grep -E "Tests *[0-9]+ passed" | head -1
pnpm -F @cliptown/adapter-opencode test 2>&1 | grep -E "Tests *[0-9]+ passed" | head -1
pnpm -F @cliptown/worker test 2>&1 | grep -E "Tests *[0-9]+ passed" | head -1
pnpm -F @cliptown/frontend e2e 2>&1 | tail -2 | head -1
node bench/check.mjs 2>&1 | python3 -c "import json,sys; d=json.load(sys.stdin); print(f'check.mjs ok={d[\"ok\"]}')"
```

Expected:
- rust: 245 (was 231, +8 skills unit +3 skills_integration +3 api_skills)
- adapter-core: 3
- adapter-claude-code: 8
- adapter-codex: 12
- adapter-opencode: 12
- worker: 75 (was 70, +3 skills_client +2 execenv extension)
- frontend e2e: 14 passed
- check.mjs: ok=True

No commit in this step — pure verification.

---

## Definition of done

- Migration `0002_skills.sql` applies; `skills` + `agent_skills` tables exist with cascade FKs.
- `crates/world/src/skills.rs` DAO with 8 inline unit tests green.
- 5 MCP tools registered + dispatched + handled; catalog 16 → 21.
- HTTP endpoint `/api/agents/:agent_id/skills` mounted with bearer auth.
- 3 MCP integration tests + 3 HTTP integration tests green.
- Worker `skills_client.ts` with 3 unit tests + `prepareWorkdir` extension with 2 added tests, all green.
- `--real` branch fetches skills + materializes them in execenv; warn-and-continue on fetch failure.
- Smoke seeds + attaches a skill, verifies on-disk landing + CLAUDE.md reference.
- Test totals: rust 245, worker 75, adapters 35, frontend e2e 14, bench gate ok.
- CHANGELOG carries M12 P2.2 section + known-limitations bullet list. TODOS Completed has matching entry (PR # filled at PR-create).
