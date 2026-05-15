//! P3 carry-forward: skill revision history tests.

use cliptown_world::{skills, storage};

async fn fixture() -> (sqlx::SqlitePool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = storage::open(dir.path().join("t.db").to_str().unwrap())
        .await
        .unwrap();
    cliptown_world::seed::seed_if_empty(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'a', 'g', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    (pool, dir)
}

#[tokio::test]
async fn first_upsert_creates_revision_seq_1() {
    let (pool, _dir) = fixture().await;
    let (id, _) = skills::upsert(&pool, "s1", "my-skill", "v1").await.unwrap();
    let revs = skills::list_revisions(&pool, "s1", &id).await.unwrap();
    assert_eq!(revs.len(), 1);
    assert_eq!(revs[0].rev_seq, 1);
    assert_eq!(revs[0].content_md, "v1");
}

#[tokio::test]
async fn subsequent_upserts_increment_rev_seq() {
    let (pool, _dir) = fixture().await;
    let (id, _) = skills::upsert(&pool, "s1", "my-skill", "v1").await.unwrap();
    skills::upsert(&pool, "s1", "my-skill", "v2").await.unwrap();
    skills::upsert(&pool, "s1", "my-skill", "v3").await.unwrap();
    let revs = skills::list_revisions(&pool, "s1", &id).await.unwrap();
    assert_eq!(revs.len(), 3);
    // list_revisions returns newest-first.
    assert_eq!(revs[0].rev_seq, 3);
    assert_eq!(revs[0].content_md, "v3");
    assert_eq!(revs[1].rev_seq, 2);
    assert_eq!(revs[2].rev_seq, 1);
}

#[tokio::test]
async fn author_agent_recorded() {
    let (pool, _dir) = fixture().await;
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('e1','s1','E1','engineer','claude_code','m','{}','suite_1','idle')"
    ).execute(&pool).await.unwrap();
    let (id, _) = skills::upsert_with_author(
        &pool, "s1", "my-skill", "v1",
        skills::Author::Agent("e1"),
    ).await.unwrap();
    let revs = skills::list_revisions(&pool, "s1", &id).await.unwrap();
    assert_eq!(revs[0].created_by_agent_id.as_deref(), Some("e1"));
    assert!(revs[0].created_by_operator_id.is_none());
}

#[tokio::test]
async fn author_operator_recorded() {
    let (pool, _dir) = fixture().await;
    // Migration 0003 seeds op_default.
    let (id, _) = skills::upsert_with_author(
        &pool, "s1", "my-skill", "v1",
        skills::Author::Operator("op_default"),
    ).await.unwrap();
    let revs = skills::list_revisions(&pool, "s1", &id).await.unwrap();
    assert!(revs[0].created_by_agent_id.is_none());
    assert_eq!(revs[0].created_by_operator_id.as_deref(), Some("op_default"));
}

#[tokio::test]
async fn list_revisions_cross_startup_rejected() {
    let (pool, _dir) = fixture().await;
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s2', 'b', 'g', 10.0, 'town_default', '/tmp/s2', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    let (id, _) = skills::upsert(&pool, "s1", "k", "x").await.unwrap();
    let r = skills::list_revisions(&pool, "s2", &id).await;
    assert!(matches!(r, Err(skills::SkillError::CrossStartup)));
}

#[tokio::test]
async fn list_revisions_unknown_skill_returns_not_found() {
    let (pool, _dir) = fixture().await;
    let r = skills::list_revisions(&pool, "s1", "ghost-id").await;
    assert!(matches!(r, Err(skills::SkillError::NotFound)));
}

#[tokio::test]
async fn skill_delete_cascades_revisions() {
    let (pool, _dir) = fixture().await;
    let (id, _) = skills::upsert(&pool, "s1", "k", "v1").await.unwrap();
    skills::upsert(&pool, "s1", "k", "v2").await.unwrap();
    // 2 revisions exist; deletion cascades.
    skills::delete(&pool, "s1", &id).await.unwrap();
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM skill_revisions WHERE skill_id = ?")
        .bind(&id).fetch_one(&pool).await.unwrap();
    assert_eq!(count.0, 0);
}
