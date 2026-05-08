use cliptown_world::{persist, seed, storage};
use sqlx::Row;

async fn fresh_db_with_seed() -> sqlx::SqlitePool {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let pool = storage::open(path.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    std::mem::forget(dir); // keep tempdir alive for the test duration
    pool
}

async fn insert_minimal_startup_and_agent_and_task(pool: &sqlx::SqlitePool) {
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) VALUES ('s1', 'alpha', 'goal', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
    ).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) VALUES ('a1', 's1', 'A1', 'engineer', 'claude_code', 'claude-3-5-sonnet', '{\"x\":3,\"y\":2,\"room\":\"suite_1\"}', 'suite_1', 'idle')"
    ).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, created_at, updated_at) VALUES ('T1', 's1', 'task one', 'desc', 'queued', unixepoch(), unixepoch())"
    ).execute(pool).await.unwrap();
}

#[tokio::test]
async fn append_audit_grows_array() {
    let pool = fresh_db_with_seed().await;
    insert_minimal_startup_and_agent_and_task(&pool).await;
    persist::append_audit(&pool, "T1", r#"{"kind":"task_assigned","ts":1}"#).await.unwrap();
    persist::append_audit(&pool, "T1", r#"{"kind":"task_done","ts":2}"#).await.unwrap();
    let row = sqlx::query("SELECT json_array_length(audit_trail) AS n FROM tasks WHERE id='T1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(row.get::<i64, _>("n"), 2);
}

#[tokio::test]
async fn append_epistemic_grows_array() {
    let pool = fresh_db_with_seed().await;
    insert_minimal_startup_and_agent_and_task(&pool).await;
    persist::append_epistemic(&pool, "T1", r#"{"kind":"hypothesis","id":"H1"}"#).await.unwrap();
    persist::append_epistemic(&pool, "T1", r#"{"kind":"test","outcome":"pass"}"#).await.unwrap();
    persist::append_epistemic(&pool, "T1", r#"{"kind":"hypothesis_resolve","status":"verified"}"#).await.unwrap();
    let row = sqlx::query("SELECT json_array_length(epistemic_log) AS n FROM tasks WHERE id='T1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(row.get::<i64, _>("n"), 3);
}

#[tokio::test]
async fn record_fs_audit_writes_row() {
    let pool = fresh_db_with_seed().await;
    insert_minimal_startup_and_agent_and_task(&pool).await;
    persist::record_fs_audit(&pool, "s1", "a1", "write", "artifacts/T1.md", 142, true, None).await.unwrap();
    persist::record_fs_audit(&pool, "s1", "a1", "write", "../escape", 0, false, Some("path escapes root")).await.unwrap();
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM fs_audit").fetch_one(&pool).await.unwrap();
    assert_eq!(count.0, 2);
    let failed: (i64,) = sqlx::query_as("SELECT count(*) FROM fs_audit WHERE ok=0").fetch_one(&pool).await.unwrap();
    assert_eq!(failed.0, 1);
}

#[tokio::test]
async fn record_budget_event_writes_row() {
    let pool = fresh_db_with_seed().await;
    insert_minimal_startup_and_agent_and_task(&pool).await;
    persist::record_budget_event(&pool, "s1", "a1", Some("T1"), 1500, 800, 0.012, "claude-3-5-sonnet").await.unwrap();
    let row = sqlx::query("SELECT in_tokens, out_tokens, cost_usd FROM budget_events WHERE startup_id='s1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(row.get::<i64, _>("in_tokens"), 1500);
    assert_eq!(row.get::<i64, _>("out_tokens"), 800);
    assert!((row.get::<f64, _>("cost_usd") - 0.012).abs() < 1e-9);
}

#[tokio::test]
async fn record_system_event_writes_row() {
    let pool = fresh_db_with_seed().await;
    persist::record_system_event(&pool, Some("s1"), "budget_warning", r#"{"percent":80}"#, "warn").await.unwrap();
    persist::record_system_event(&pool, None, "world_started", r#"{}"#, "info").await.unwrap();
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM system_events").fetch_one(&pool).await.unwrap();
    assert_eq!(count.0, 2);
}

#[tokio::test]
async fn snapshot_positions_updates_agents() {
    let pool = fresh_db_with_seed().await;
    insert_minimal_startup_and_agent_and_task(&pool).await;
    let updates = vec![
        ("a1".to_string(), r#"{"x":10,"y":5,"room":"cafe"}"#.to_string()),
    ];
    persist::snapshot_positions(&pool, &updates).await.unwrap();
    let row: (String,) = sqlx::query_as("SELECT position_json FROM agents WHERE id='a1'")
        .fetch_one(&pool).await.unwrap();
    assert!(row.0.contains("cafe"));
    assert!(row.0.contains("\"x\":10"));
}

#[tokio::test]
async fn snapshot_positions_empty_is_noop() {
    let pool = fresh_db_with_seed().await;
    insert_minimal_startup_and_agent_and_task(&pool).await;
    persist::snapshot_positions(&pool, &[]).await.unwrap();
    // Original position retained.
    let row: (String,) = sqlx::query_as("SELECT position_json FROM agents WHERE id='a1'")
        .fetch_one(&pool).await.unwrap();
    assert!(row.0.contains("\"x\":3"));
}
