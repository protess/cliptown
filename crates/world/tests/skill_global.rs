//! P3 carry-forward: globally-visible skills.

use cliptown_world::{skills, storage};

async fn setup() -> (sqlx::SqlitePool, tempfile::TempDir) {
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
    for (aid, sid) in [("e1", "s1"), ("e2", "s2")] {
        sqlx::query(
            "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
             VALUES (?, ?, 'A', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle')"
        ).bind(aid).bind(sid).execute(&pool).await.unwrap();
    }
    (pool, dir)
}

#[tokio::test]
async fn for_agent_includes_globally_marked_skill_without_attachment() {
    let (pool, _dir) = setup().await;
    // Skill owned by s1 but marked global.
    let (id, _) = skills::upsert(&pool, "s1", "style-guide", "be concise").await.unwrap();
    skills::set_global(&pool, &id, true).await.unwrap();
    // s2's agent has no agent_skills row but should still see it.
    let attached = skills::for_agent(&pool, "e2").await.unwrap();
    assert_eq!(attached.len(), 1);
    assert_eq!(attached[0].name, "style-guide");
}

#[tokio::test]
async fn for_agent_does_not_double_list_globally_attached_skill() {
    let (pool, _dir) = setup().await;
    let (id, _) = skills::upsert(&pool, "s1", "k", "v").await.unwrap();
    skills::set_global(&pool, &id, true).await.unwrap();
    // e1 explicitly attaches the same skill (in its owning startup).
    sqlx::query("INSERT INTO agent_skills (agent_id, skill_id, attached_at) VALUES ('e1', ?, unixepoch())")
        .bind(&id).execute(&pool).await.unwrap();
    let attached = skills::for_agent(&pool, "e1").await.unwrap();
    assert_eq!(attached.len(), 1, "global + attached should not duplicate: {attached:?}");
}

#[tokio::test]
async fn clearing_global_hides_skill_from_other_startups() {
    let (pool, _dir) = setup().await;
    let (id, _) = skills::upsert(&pool, "s1", "k", "v").await.unwrap();
    skills::set_global(&pool, &id, true).await.unwrap();
    assert_eq!(skills::for_agent(&pool, "e2").await.unwrap().len(), 1);
    skills::set_global(&pool, &id, false).await.unwrap();
    assert_eq!(skills::for_agent(&pool, "e2").await.unwrap().len(), 0,
        "after clearing global, cross-startup agent shouldn't see it");
}

#[tokio::test]
async fn set_global_on_unknown_skill_returns_not_found() {
    let (pool, _dir) = setup().await;
    let r = skills::set_global(&pool, "ghost", true).await;
    assert!(matches!(r, Err(skills::SkillError::NotFound)));
}
