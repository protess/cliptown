//! P5 Theme F: hot-snapshot + restore-drill integration tests.

use cliptown_world::backup::{prune, snapshot_filename, snapshot_to, BackupError};
use cliptown_world::{seed, storage};
use std::fs;

#[tokio::test]
async fn snapshot_to_creates_a_valid_sqlite_file() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live.db");
    let snap = dir.path().join("snap.db");
    let pool = storage::open(live.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1','a','g',10.0,'town_default','/tmp/s1','active',unixepoch())"
    ).execute(&pool).await.unwrap();

    snapshot_to(&pool, &snap).await.expect("snapshot should succeed");
    assert!(snap.exists(), "snapshot file must exist");
    let size = fs::metadata(&snap).unwrap().len();
    assert!(size > 0, "snapshot must have content");

    // The snapshot itself must be a valid SQLite DB: open it and
    // query the row we inserted.
    let snap_pool = storage::open(snap.to_str().unwrap()).await.unwrap();
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM startups WHERE id='s1'")
        .fetch_one(&snap_pool).await.unwrap();
    assert_eq!(count.0, 1);
}

#[tokio::test]
async fn snapshot_rejects_path_with_single_quote() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live.db");
    let bad = dir.path().join("bad'name.db");
    let pool = storage::open(live.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    let err = snapshot_to(&pool, &bad).await.unwrap_err();
    assert!(matches!(err, BackupError::UnsafePath(_)), "got {err}");
}

/// Restore drill: snapshot → mutate → swap files → re-open → assert
/// state matches the snapshot, not the post-mutation live DB.
#[tokio::test]
async fn restore_from_snapshot_rolls_back_state() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live.db");
    let snap = dir.path().join("snap.db");

    // 1. Boot, seed, write "before" state.
    let pool = storage::open(live.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s_before','a','g',10.0,'town_default','/tmp/s_before','active',unixepoch())"
    ).execute(&pool).await.unwrap();

    // 2. Take a snapshot at this point.
    snapshot_to(&pool, &snap).await.unwrap();

    // 3. Mutate post-snapshot. This is the work that the restore must undo.
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s_after','b','g',10.0,'town_default','/tmp/s_after','active',unixepoch())"
    ).execute(&pool).await.unwrap();

    // 4. Close the live pool so SQLite releases the WAL handle (simulates
    //    stopping the world process before running the restore script).
    //    `close().await` is explicit — bare `drop(pool)` doesn't wait
    //    for the async connection cleanup to flush, which races with the
    //    file swap below when the test suite is heavily threaded.
    pool.close().await;
    drop(pool);

    // 5. Swap snapshot over live + remove stale WAL/SHM (mirrors what
    //    scripts/restore-from-snapshot.sh does).
    fs::copy(&snap, &live).unwrap();
    for ext in ["-wal", "-shm"] {
        let mut p = live.clone();
        let s = p.to_str().unwrap().to_string() + ext;
        p = std::path::PathBuf::from(s);
        if p.exists() {
            fs::remove_file(&p).unwrap();
        }
    }

    // 6. Re-open and assert: only `s_before` survives.
    let pool2 = storage::open(live.to_str().unwrap()).await.unwrap();
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM startups WHERE id='s_before'")
        .fetch_one(&pool2).await.unwrap();
    assert_eq!(count.0, 1, "s_before must survive the restore");
    let after: (i64,) = sqlx::query_as("SELECT count(*) FROM startups WHERE id='s_after'")
        .fetch_one(&pool2).await.unwrap();
    assert_eq!(after.0, 0, "s_after must be rolled back");
}

#[test]
fn snapshot_filename_uses_timestamp_format() {
    let dir = std::path::PathBuf::from("/var/backups/cliptown");
    let when = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
    let p = snapshot_filename(&dir, when);
    let s = p.to_string_lossy();
    assert!(s.starts_with("/var/backups/cliptown/cliptown-"));
    assert!(s.ends_with(".db"));
}

#[test]
fn prune_keeps_newest_n_and_deletes_rest() {
    let dir = tempfile::tempdir().unwrap();
    // Create 5 fake snapshots; only 2 should survive a keep=2 sweep.
    let names = [
        "cliptown-20260101-000000.db",
        "cliptown-20260102-000000.db",
        "cliptown-20260103-000000.db",
        "cliptown-20260104-000000.db",
        "cliptown-20260105-000000.db",
    ];
    for n in names {
        fs::write(dir.path().join(n), b"x").unwrap();
    }
    // Sentinel non-snapshot file — must NOT be touched.
    fs::write(dir.path().join("README.txt"), b"hi").unwrap();
    let deleted = prune(dir.path(), 2).unwrap();
    assert_eq!(deleted, 3);
    assert!(dir.path().join("cliptown-20260105-000000.db").exists());
    assert!(dir.path().join("cliptown-20260104-000000.db").exists());
    assert!(!dir.path().join("cliptown-20260101-000000.db").exists());
    assert!(dir.path().join("README.txt").exists(), "non-snapshots must survive");
}

#[test]
fn prune_with_missing_dir_is_noop() {
    let n = prune(std::path::Path::new("/tmp/this-does-not-exist-cliptown-zzz"), 5).unwrap();
    assert_eq!(n, 0);
}
