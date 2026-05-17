//! P6 Theme A: self-review check pipeline tests.

use cliptown_world::self_review::{canonical_artifact_path, record, run, Severity};
use cliptown_world::{seed, storage};
use std::fs;

async fn ctx() -> (sqlx::SqlitePool, tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let pool = storage::open(dir.path().join("t.db").to_str().unwrap())
        .await
        .unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1','a','g',10.0,'town_default','/tmp/s1','active',unixepoch())"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('e1', 's1', 'E1', 'engineer', 'claude_code', 'm', '{}', 'suite_1', 'idle')"
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 't', 'd', 'in_progress', 'e1', unixepoch(), unixepoch())"
    ).execute(&pool).await.unwrap();
    (pool, dir, cwd)
}

fn restore_cwd(saved: &std::path::Path) {
    let _ = std::env::set_current_dir(saved);
}

/// Serialize the env-CWD-mutating tests so they don't trample each other.
/// Tokio multi-thread runtime would interleave them otherwise.
static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tokio::test]
async fn run_passes_on_canonical_path_with_existing_artifact() {
    let _g = CWD_LOCK.lock().unwrap();
    let (_pool, dir, cwd) = ctx().await;
    let artifacts = dir.path().join("workspaces/s1/artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    fs::write(artifacts.join("T1.md"), "the artifact body").unwrap();

    let outcome = run("s1", "T1", "workspaces/s1/artifacts/T1.md").await;
    assert!(outcome.ok, "ok=false: {:?}", outcome.must_fix);
    // Markdown-lint stub still emits a warn-severity entry.
    assert!(
        outcome.must_fix.iter().all(|c| c.severity == Severity::Warn),
        "expected only warn-severity entries, got {:?}",
        outcome.must_fix
    );
    restore_cwd(&cwd);
}

#[tokio::test]
async fn run_fails_on_non_canonical_path() {
    let _g = CWD_LOCK.lock().unwrap();
    let (_pool, _dir, cwd) = ctx().await;
    let outcome = run("s1", "T1", "wrong/place/T1.md").await;
    assert!(!outcome.ok);
    assert!(outcome.must_fix.iter().any(|c| c.check == "canonical_path"
        && c.severity == Severity::Error));
    restore_cwd(&cwd);
}

#[tokio::test]
async fn run_fails_when_artifact_missing() {
    let _g = CWD_LOCK.lock().unwrap();
    let (_pool, _dir, cwd) = ctx().await;
    // No file created at the canonical path.
    let outcome = run("s1", "T1", "workspaces/s1/artifacts/T1.md").await;
    assert!(!outcome.ok);
    assert!(outcome.must_fix.iter().any(|c| c.check == "artifact_exists"
        && c.severity == Severity::Error));
    restore_cwd(&cwd);
}

#[tokio::test]
async fn run_fails_on_empty_artifact() {
    let _g = CWD_LOCK.lock().unwrap();
    let (_pool, dir, cwd) = ctx().await;
    let artifacts = dir.path().join("workspaces/s1/artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    fs::write(artifacts.join("T1.md"), "").unwrap();

    let outcome = run("s1", "T1", "workspaces/s1/artifacts/T1.md").await;
    assert!(!outcome.ok);
    assert!(outcome.must_fix.iter().any(|c| c.check == "artifact_exists"
        && c.message.contains("zero bytes")));
    restore_cwd(&cwd);
}

#[tokio::test]
async fn record_stamps_self_reviewed_at_only_on_pass() {
    let _g = CWD_LOCK.lock().unwrap();
    let (pool, dir, cwd) = ctx().await;
    let artifacts = dir.path().join("workspaces/s1/artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    fs::write(artifacts.join("T1.md"), "x").unwrap();
    let outcome = run("s1", "T1", "workspaces/s1/artifacts/T1.md").await;
    assert!(outcome.ok);
    record(&pool, "T1", "e1", &outcome).await.unwrap();
    let stamp: (Option<i64>,) = sqlx::query_as("SELECT self_reviewed_at FROM tasks WHERE id='T1'")
        .fetch_one(&pool).await.unwrap();
    assert!(stamp.0.is_some(), "stamp must be set on pass");

    // Now a failing run must NOT clobber the timestamp into the future.
    sqlx::query("UPDATE tasks SET self_reviewed_at = NULL WHERE id='T1'")
        .execute(&pool).await.unwrap();
    let fail = run("s1", "T1", "wrong/path/T1.md").await;
    assert!(!fail.ok);
    record(&pool, "T1", "e1", &fail).await.unwrap();
    let stamp_after: (Option<i64>,) = sqlx::query_as("SELECT self_reviewed_at FROM tasks WHERE id='T1'")
        .fetch_one(&pool).await.unwrap();
    assert!(stamp_after.0.is_none(), "fail must not stamp");
    restore_cwd(&cwd);
}

#[test]
fn canonical_artifact_path_format() {
    assert_eq!(
        canonical_artifact_path("s1", "T1"),
        "workspaces/s1/artifacts/T1.md"
    );
}
