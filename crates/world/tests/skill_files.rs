//! P3 carry-forward: skill file attachments — DAO + cross-startup guard tests.

use cliptown_world::{skills, storage};

async fn fixture() -> (sqlx::SqlitePool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = storage::open(dir.path().join("t.db").to_str().unwrap())
        .await
        .unwrap();
    cliptown_world::seed::seed_if_empty(&pool).await.unwrap();
    for (sid, name) in [("s1", "alpha"), ("s2", "beta")] {
        sqlx::query(
            "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
             VALUES (?, ?, 'g', 10.0, 'town_default', '/tmp/ws', 'active', unixepoch())"
        ).bind(sid).bind(name).execute(&pool).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO skills (id, startup_id, name, content_md, created_at, updated_at) \
         VALUES ('sk1', 's1', 'deploy', 'deploy md', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();
    (pool, dir)
}

#[tokio::test]
async fn upsert_then_list_round_trip() {
    let (pool, _dir) = fixture().await;
    let id = skills::upsert_file(&pool, "s1", "sk1", "template.txt", "hello").await.unwrap();
    assert!(!id.is_empty());
    let files = skills::list_files(&pool, "sk1").await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name, "template.txt");
    assert_eq!(files[0].content, "hello");
}

#[tokio::test]
async fn upsert_replaces_existing_by_name() {
    let (pool, _dir) = fixture().await;
    let id1 = skills::upsert_file(&pool, "s1", "sk1", "f", "v1").await.unwrap();
    let id2 = skills::upsert_file(&pool, "s1", "sk1", "f", "v2").await.unwrap();
    assert_eq!(id1, id2, "same name → updates in place, no new row");
    let files = skills::list_files(&pool, "sk1").await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].content, "v2");
}

#[tokio::test]
async fn cross_startup_rejected() {
    let (pool, _dir) = fixture().await;
    // s2 caller trying to attach a file to s1's skill.
    let r = skills::upsert_file(&pool, "s2", "sk1", "f", "x").await;
    assert!(matches!(r, Err(skills::SkillError::CrossStartup)));
}

#[tokio::test]
async fn delete_removes_row() {
    let (pool, _dir) = fixture().await;
    skills::upsert_file(&pool, "s1", "sk1", "f", "x").await.unwrap();
    skills::delete_file(&pool, "s1", "sk1", "f").await.unwrap();
    assert!(skills::list_files(&pool, "sk1").await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_missing_returns_not_found() {
    let (pool, _dir) = fixture().await;
    let r = skills::delete_file(&pool, "s1", "sk1", "nope").await;
    assert!(matches!(r, Err(skills::SkillError::NotFound)));
}

#[tokio::test]
async fn skill_delete_cascades_files() {
    let (pool, _dir) = fixture().await;
    skills::upsert_file(&pool, "s1", "sk1", "a", "1").await.unwrap();
    skills::upsert_file(&pool, "s1", "sk1", "b", "2").await.unwrap();
    skills::delete(&pool, "s1", "sk1").await.unwrap();
    // skill_files has FK ON DELETE CASCADE — should be gone.
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM skill_files WHERE skill_id = 'sk1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn bad_file_names_rejected() {
    let (pool, _dir) = fixture().await;
    for bad in &["", "../escape", "with/slash", "..", ".", "has space"] {
        let r = skills::upsert_file(&pool, "s1", "sk1", bad, "x").await;
        assert!(matches!(r, Err(skills::SkillError::BadName)), "should reject {bad:?}, got {r:?}");
    }
    for good in &["template.txt", "config_v1.json", "READ-ME.md"] {
        let r = skills::upsert_file(&pool, "s1", "sk1", good, "x").await;
        assert!(r.is_ok(), "should accept {good:?}, got {r:?}");
    }
}

#[tokio::test]
async fn for_agent_includes_files() {
    let (pool, _dir) = fixture().await;
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('e1','s1','E1','engineer','claude_code','m','{}','suite_1','idle')"
    ).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO agent_skills (agent_id, skill_id, attached_at) VALUES ('e1','sk1',unixepoch())")
        .execute(&pool).await.unwrap();
    skills::upsert_file(&pool, "s1", "sk1", "tmpl.txt", "hello").await.unwrap();
    let attached = skills::for_agent(&pool, "e1").await.unwrap();
    assert_eq!(attached.len(), 1);
    assert_eq!(attached[0].name, "deploy");
    assert_eq!(attached[0].files.len(), 1);
    assert_eq!(attached[0].files[0].name, "tmpl.txt");
    assert_eq!(attached[0].files[0].content, "hello");
}
