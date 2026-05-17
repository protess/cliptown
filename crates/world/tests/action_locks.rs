//! P5 Theme C: action_locks unit tests.

use cliptown_world::action_locks::{gc_expired, list_active, release, try_acquire, AcquireError, DEFAULT_TTL_SECS};
use cliptown_world::{seed, storage};

async fn fresh_pool() -> sqlx::SqlitePool {
    let dir = tempfile::tempdir().unwrap();
    let pool = storage::open(dir.path().join("t.db").to_str().unwrap())
        .await
        .unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    sqlx::query("INSERT INTO operators (id, name, token, role, created_at) VALUES ('op_a','Alice','tok_a','admin',unixepoch())")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO operators (id, name, token, role, created_at) VALUES ('op_b','Bob','tok_b','admin',unixepoch())")
        .execute(&pool).await.unwrap();
    std::mem::forget(dir);
    pool
}

#[tokio::test]
async fn try_acquire_succeeds_for_first_caller() {
    let pool = fresh_pool().await;
    let info = try_acquire(&pool, "task:T1:force_accept", "op_a", DEFAULT_TTL_SECS)
        .await
        .expect("first acquire should succeed");
    assert_eq!(info.lock_key, "task:T1:force_accept");
    assert_eq!(info.operator_id, "op_a");
    assert_eq!(info.operator_name, "Alice");
}

#[tokio::test]
async fn try_acquire_conflicts_on_held_key() {
    let pool = fresh_pool().await;
    let _ = try_acquire(&pool, "task:T1:force_accept", "op_a", DEFAULT_TTL_SECS).await.unwrap();
    let err = try_acquire(&pool, "task:T1:force_accept", "op_b", DEFAULT_TTL_SECS).await.unwrap_err();
    let conflict = match err {
        AcquireError::Conflict(c) => c,
        AcquireError::Sql(e) => panic!("unexpected sql: {e}"),
    };
    assert_eq!(conflict.operator_id, "op_a");
    assert_eq!(conflict.operator_name, "Alice");
}

#[tokio::test]
async fn release_removes_lock_and_allows_reacquire() {
    let pool = fresh_pool().await;
    let _ = try_acquire(&pool, "task:T1:force_accept", "op_a", DEFAULT_TTL_SECS).await.unwrap();
    assert!(release(&pool, "task:T1:force_accept").await.unwrap());
    // op_b can now acquire fresh.
    let info = try_acquire(&pool, "task:T1:force_accept", "op_b", DEFAULT_TTL_SECS).await.unwrap();
    assert_eq!(info.operator_id, "op_b");
}

#[tokio::test]
async fn try_acquire_replaces_stale_lock_from_crashed_peer() {
    let pool = fresh_pool().await;
    // Manually insert an expired row to simulate a crashed peer's
    // wedged lock. try_acquire's eager sweep should clear it.
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO action_locks (id, lock_key, operator_id, acquired_at, expires_at) \
         VALUES ('lock1', 'task:T1:force_accept', 'op_a', ?, ?)"
    ).bind(now - 60).bind(now - 30).execute(&pool).await.unwrap();
    let info = try_acquire(&pool, "task:T1:force_accept", "op_b", DEFAULT_TTL_SECS).await.unwrap();
    assert_eq!(info.operator_id, "op_b", "stale lock must be swept on acquire");
}

#[tokio::test]
async fn gc_expired_returns_dropped_keys() {
    let pool = fresh_pool().await;
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO action_locks (id, lock_key, operator_id, acquired_at, expires_at) \
         VALUES ('l1', 'task:T1:force_accept', 'op_a', ?, ?)"
    ).bind(now - 60).bind(now - 30).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO action_locks (id, lock_key, operator_id, acquired_at, expires_at) \
         VALUES ('l2', 'operator:op_c:revoke', 'op_b', ?, ?)"
    ).bind(now).bind(now + 30).execute(&pool).await.unwrap();

    let dropped = gc_expired(&pool, now).await.unwrap();
    assert_eq!(dropped, vec!["task:T1:force_accept".to_string()]);
    // The live one must survive.
    let active = list_active(&pool, now).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].lock_key, "operator:op_c:revoke");
}

#[tokio::test]
async fn list_active_filters_expired() {
    let pool = fresh_pool().await;
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO action_locks (id, lock_key, operator_id, acquired_at, expires_at) \
         VALUES ('l_stale', 'task:T1:force_accept', 'op_a', ?, ?)"
    ).bind(now - 60).bind(now - 30).execute(&pool).await.unwrap();
    let active = list_active(&pool, now).await.unwrap();
    assert!(active.is_empty(), "list_active must skip expired rows");
}
