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
    /// P3 carry-forward: associated text files. Worker writes each to
    /// `<workdir>/skills/<skill-name>/<file-name>` alongside the main `.md`.
    /// Empty when the skill has no attached files (default).
    pub files: Vec<SkillFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFile {
    pub name: String,
    pub content: String,
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

pub async fn upsert(
    pool: &SqlitePool,
    startup_id: &str,
    name: &str,
    content_md: &str,
) -> Result<(String, bool), SkillError> {
    upsert_with_author(pool, startup_id, name, content_md, Author::Unknown).await
}

/// P3 carry-forward: identity of the caller for the revision-history log.
/// Most call paths know whether they're agent-side (MCP tool) or operator-
/// side (Console). The `Unknown` variant exists for test fixtures + the
/// legacy `upsert` shim above so callers can migrate incrementally.
#[derive(Debug, Clone)]
pub enum Author<'a> {
    Agent(&'a str),
    Operator(&'a str),
    Unknown,
}

pub async fn upsert_with_author<'a>(
    pool: &SqlitePool,
    startup_id: &str,
    name: &str,
    content_md: &str,
    author: Author<'a>,
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
    let (id, is_new) = match existing {
        Some((id,)) => {
            sqlx::query(
                "UPDATE skills SET content_md = ?, updated_at = unixepoch() WHERE id = ?",
            )
            .bind(content_md)
            .bind(&id)
            .execute(pool)
            .await?;
            (id, false)
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
            (id, true)
        }
    };
    // P3 carry-forward: write the revision row AFTER the live update so the
    // sequence count includes the new version. Failure here logs but doesn't
    // unwind the live update — losing history is preferable to losing the
    // user's authored content.
    if let Err(e) = append_revision(pool, &id, content_md, author).await {
        tracing::warn!(
            component = "skills",
            skill_id = %id,
            err = ?e,
            "failed to append skill_revisions row; live skill update kept"
        );
    }
    Ok((id, is_new))
}

async fn append_revision<'a>(
    pool: &SqlitePool,
    skill_id: &str,
    content_md: &str,
    author: Author<'a>,
) -> Result<(), SkillError> {
    let next_seq: (i64,) = sqlx::query_as(
        "SELECT COALESCE(MAX(rev_seq), 0) + 1 FROM skill_revisions WHERE skill_id = ?",
    )
    .bind(skill_id)
    .fetch_one(pool)
    .await?;
    let (agent, operator) = match author {
        Author::Agent(id) => (Some(id), None),
        Author::Operator(id) => (None, Some(id)),
        Author::Unknown => (None, None),
    };
    let rev_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO skill_revisions (id, skill_id, rev_seq, content_md, created_at, \
                                       created_by_agent_id, created_by_operator_id) \
         VALUES (?, ?, ?, ?, unixepoch(), ?, ?)",
    )
    .bind(&rev_id)
    .bind(skill_id)
    .bind(next_seq.0)
    .bind(content_md)
    .bind(agent)
    .bind(operator)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRevision {
    pub id: String,
    pub skill_id: String,
    pub rev_seq: i64,
    pub content_md: String,
    pub created_at: i64,
    pub created_by_agent_id: Option<String>,
    pub created_by_operator_id: Option<String>,
}

pub async fn list_revisions(
    pool: &SqlitePool,
    startup_id: &str,
    skill_id: &str,
) -> Result<Vec<SkillRevision>, SkillError> {
    // Ownership gate so a cross-startup caller can't peek at content history.
    let owner: Option<(String,)> =
        sqlx::query_as("SELECT startup_id FROM skills WHERE id = ?")
            .bind(skill_id)
            .fetch_optional(pool)
            .await?;
    match owner {
        None => return Err(SkillError::NotFound),
        Some((sid,)) if sid != startup_id => return Err(SkillError::CrossStartup),
        _ => {}
    }
    let rows: Vec<(String, String, i64, String, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, skill_id, rev_seq, content_md, created_at, \
                created_by_agent_id, created_by_operator_id \
         FROM skill_revisions WHERE skill_id = ? ORDER BY rev_seq DESC",
    )
    .bind(skill_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, skill_id, rev_seq, content_md, created_at, agent, operator)| SkillRevision {
            id,
            skill_id,
            rev_seq,
            content_md,
            created_at,
            created_by_agent_id: agent,
            created_by_operator_id: operator,
        })
        .collect())
}

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

pub async fn detach(
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
    sqlx::query("DELETE FROM agent_skills WHERE agent_id = ? AND skill_id = ?")
        .bind(agent_id)
        .bind(skill_id)
        .execute(pool)
        .await?;
    Ok(())
}

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

pub async fn for_agent(
    pool: &SqlitePool,
    agent_id: &str,
) -> Result<Vec<AttachedSkill>, SkillError> {
    // P3 carry-forward (file attachments + global): union explicitly-
    // attached skills with `is_global = 1` rows, surfacing skill_id so we
    // can fetch attached files per row. DISTINCT-by-id de-dups skills that
    // are both attached AND global.
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, name, content_md FROM ( \
            SELECT DISTINCT s.id, s.name, s.content_md FROM skills s \
            LEFT JOIN agent_skills ags ON ags.skill_id = s.id AND ags.agent_id = ? \
            WHERE ags.agent_id IS NOT NULL OR s.is_global = 1 \
         ) ORDER BY name",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await?;
    let mut out: Vec<AttachedSkill> = Vec::with_capacity(rows.len());
    for (id, name, content_md) in rows {
        let files = list_files(pool, &id).await.unwrap_or_default();
        out.push(AttachedSkill { name, content_md, files });
    }
    Ok(out)
}

/// P3 carry-forward: file CRUD on `skill_files`.
pub async fn list_files(
    pool: &SqlitePool,
    skill_id: &str,
) -> Result<Vec<SkillFile>, SkillError> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, content FROM skill_files WHERE skill_id = ? ORDER BY name",
    )
    .bind(skill_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(name, content)| SkillFile { name, content })
        .collect())
}

/// P3 carry-forward: set or clear the `is_global` flag on a skill.
/// Caller (operator) must be admin; enforced at the dispatch layer.
pub async fn set_global(
    pool: &SqlitePool,
    skill_id: &str,
    is_global: bool,
) -> Result<(), SkillError> {
    let r = sqlx::query(
        "UPDATE skills SET is_global = ?, updated_at = unixepoch() WHERE id = ?",
    )
    .bind(if is_global { 1i64 } else { 0i64 })
    .bind(skill_id)
    .execute(pool)
    .await?;
    if r.rows_affected() == 0 {
        return Err(SkillError::NotFound);
    }
    Ok(())
}

/// File-name validator: same character set as skill names — lowercase alpha-
/// numeric + dash + dot + underscore. Forbids slashes and `..` segments to
/// keep the worker-side writer trivially path-safe.
pub fn file_name_is_valid(name: &str) -> bool {
    if name.is_empty() || name.len() > 128 { return false; }
    if name == "." || name == ".." || name.starts_with('/') || name.contains("..") || name.contains('/') {
        return false;
    }
    name.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
    })
}

pub async fn upsert_file(
    pool: &SqlitePool,
    startup_id: &str,
    skill_id: &str,
    name: &str,
    content: &str,
) -> Result<String, SkillError> {
    if !file_name_is_valid(name) {
        return Err(SkillError::BadName);
    }
    if content.len() > MAX_CONTENT_LEN {
        return Err(SkillError::OversizeContent);
    }
    // Ownership check: the caller's `startup_id` must own this skill row.
    // Without this an operator could overwrite files on someone else's skill
    // if they could guess a UUID.
    let owner: Option<(String,)> =
        sqlx::query_as("SELECT startup_id FROM skills WHERE id = ?")
            .bind(skill_id)
            .fetch_optional(pool)
            .await?;
    match owner {
        None => return Err(SkillError::NotFound),
        Some((sid,)) if sid != startup_id => return Err(SkillError::CrossStartup),
        _ => {}
    }
    let existing: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM skill_files WHERE skill_id = ? AND name = ?",
    )
    .bind(skill_id)
    .bind(name)
    .fetch_optional(pool)
    .await?;
    match existing {
        Some((id,)) => {
            sqlx::query(
                "UPDATE skill_files SET content = ?, updated_at = unixepoch() WHERE id = ?",
            )
            .bind(content)
            .bind(&id)
            .execute(pool)
            .await?;
            Ok(id)
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO skill_files (id, skill_id, name, content, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, unixepoch(), unixepoch())",
            )
            .bind(&id)
            .bind(skill_id)
            .bind(name)
            .bind(content)
            .execute(pool)
            .await?;
            Ok(id)
        }
    }
}

pub async fn delete_file(
    pool: &SqlitePool,
    startup_id: &str,
    skill_id: &str,
    file_name: &str,
) -> Result<(), SkillError> {
    let owner: Option<(String,)> =
        sqlx::query_as("SELECT startup_id FROM skills WHERE id = ?")
            .bind(skill_id)
            .fetch_optional(pool)
            .await?;
    match owner {
        None => return Err(SkillError::NotFound),
        Some((sid,)) if sid != startup_id => return Err(SkillError::CrossStartup),
        _ => {}
    }
    let r = sqlx::query("DELETE FROM skill_files WHERE skill_id = ? AND name = ?")
        .bind(skill_id)
        .bind(file_name)
        .execute(pool)
        .await?;
    if r.rows_affected() == 0 {
        return Err(SkillError::NotFound);
    }
    Ok(())
}

/// SkillsSnapshot row shape: listing metadata + the list of agent_ids
/// that have this skill attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillWithAttachments {
    pub id: String,
    pub name: String,
    pub len: i64,
    pub updated_at: i64,
    pub attachments: Vec<String>,
}

/// List all skills in a startup with their attachments. Used by the console
/// snapshot delivery + the per-mutation broadcast for `kind="upsert"`.
pub async fn list_with_attachments(
    pool: &SqlitePool,
    startup_id: &str,
) -> Result<Vec<SkillWithAttachments>, SkillError> {
    let skills: Vec<(String, String, i64, i64)> = sqlx::query_as(
        "SELECT id, name, length(content_md), updated_at FROM skills \
         WHERE startup_id = ? ORDER BY name",
    )
    .bind(startup_id)
    .fetch_all(pool)
    .await?;
    let mut out: Vec<SkillWithAttachments> = Vec::with_capacity(skills.len());
    for (id, name, len, updated_at) in skills {
        let attachments: Vec<(String,)> =
            sqlx::query_as("SELECT agent_id FROM agent_skills WHERE skill_id = ? ORDER BY agent_id")
                .bind(&id)
                .fetch_all(pool)
                .await?;
        out.push(SkillWithAttachments {
            id,
            name,
            len,
            updated_at,
            attachments: attachments.into_iter().map(|(a,)| a).collect(),
        });
    }
    Ok(out)
}

/// Fetch all (startup_id, [SkillWithAttachments]) pairs for the SkillsSnapshot
/// emitted at console connect. Returns a flat map; the caller serializes.
pub async fn list_all_with_attachments(
    pool: &SqlitePool,
) -> Result<std::collections::HashMap<String, Vec<SkillWithAttachments>>, SkillError> {
    let startups: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT startup_id FROM skills")
        .fetch_all(pool)
        .await?;
    let mut out = std::collections::HashMap::with_capacity(startups.len());
    for (sid,) in startups {
        let v = list_with_attachments(pool, &sid).await?;
        out.insert(sid, v);
    }
    Ok(out)
}

/// Build a serde JSON object from a `SkillWithAttachments` for broadcast use.
pub fn skill_with_attachments_to_json(s: &SkillWithAttachments) -> serde_json::Value {
    serde_json::json!({
        "id": s.id,
        "name": s.name,
        "len": s.len,
        "updated_at": s.updated_at,
        "attachments": s.attachments,
    })
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
        sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, budget_spent_usd, town_id, workspace_path, status, created_at) VALUES ('S1','alpha','goal',10.0,0.0,'town_default','/tmp/s1','active',unixepoch())")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO startups (id, name, goal_text, budget_cap_usd, budget_spent_usd, town_id, workspace_path, status, created_at) VALUES ('S2','beta','goal',10.0,0.0,'town_default','/tmp/s2','active',unixepoch())")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, manager_id, status) VALUES ('A1','S1','eng','engineer','claude_code','m','{\"x\":0,\"y\":0}','lobby',NULL,'idle')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, manager_id, status) VALUES ('A2','S2','eng','engineer','claude_code','m','{\"x\":0,\"y\":0}','lobby',NULL,'idle')")
            .execute(&pool).await.unwrap();
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
        assert!(matches!(
            attach(&pool, "S1", "A2", &sid).await,
            Err(SkillError::CrossStartup)
        ));
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
