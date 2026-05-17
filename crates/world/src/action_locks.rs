//! P5 Theme C: soft-locks on destructive operator actions.
//!
//! `try_acquire` is the entry point: it tries to claim a `lock_key`
//! for a 30s window. The SQL `UNIQUE(lock_key)` constraint is the
//! test-and-set primitive — if another operator already holds the
//! lock, the INSERT returns a conflict and we read back the existing
//! row to surface "locked_by".
//!
//! Lifecycle:
//!   - `try_acquire` returns `Ok(())` and the caller emits
//!     `ActionLocked` so peers see the disable.
//!   - On success the caller calls `release` which DELETEs the row
//!     and emits `ActionUnlocked`.
//!   - A periodic `gc` tick (every 5s) drops expired rows and emits
//!     unlocks for them so a stuck operator (browser crash mid-
//!     action) doesn't wedge the lock past TTL.

use serde::Serialize;
use sqlx::SqlitePool;

/// Soft-lock default TTL (seconds). Chosen at the high end of "an
/// operator is mid-confirm" — long enough to read a modal, short
/// enough to not wedge the team if a tab crashes.
pub const DEFAULT_TTL_SECS: i64 = 30;

#[derive(Debug, Clone, Serialize)]
pub struct LockConflict {
    pub lock_key: String,
    pub operator_id: String,
    pub operator_name: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LockInfo {
    pub lock_key: String,
    pub operator_id: String,
    pub operator_name: String,
    pub expires_at: i64,
}

#[derive(Debug)]
pub enum AcquireError {
    Conflict(LockConflict),
    Sql(sqlx::Error),
}

/// Try to acquire `lock_key` for `operator_id` for `ttl_secs`. On
/// conflict, returns `Err(Conflict(info))` carrying the holder's
/// name and remaining time. On SQL error, returns `Err(Sql(_))`.
///
/// First sweeps stale (expired) rows for this key — without this,
/// a crashed peer's lock would block all future acquires until the
/// GC ticks. The sweep is `DELETE WHERE lock_key = ? AND expires_at
/// < now` so it doesn't interfere with live locks held by others.
pub async fn try_acquire(
    pool: &SqlitePool,
    lock_key: &str,
    operator_id: &str,
    ttl_secs: i64,
) -> Result<LockInfo, AcquireError> {
    let now = chrono::Utc::now().timestamp();
    let expires_at = now + ttl_secs;
    // Eagerly drop any stale row on this key so a crashed peer doesn't
    // wedge it.
    let _ = sqlx::query("DELETE FROM action_locks WHERE lock_key = ? AND expires_at < ?")
        .bind(lock_key)
        .bind(now)
        .execute(pool)
        .await
        .map_err(AcquireError::Sql)?;
    let id = uuid::Uuid::new_v4().to_string();
    let insert = sqlx::query(
        "INSERT INTO action_locks (id, lock_key, operator_id, acquired_at, expires_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(lock_key)
    .bind(operator_id)
    .bind(now)
    .bind(expires_at)
    .execute(pool)
    .await;
    match insert {
        Ok(_) => {
            let name: Option<(String,)> =
                sqlx::query_as("SELECT name FROM operators WHERE id = ?")
                    .bind(operator_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(AcquireError::Sql)?;
            Ok(LockInfo {
                lock_key: lock_key.to_string(),
                operator_id: operator_id.to_string(),
                operator_name: name.map(|(n,)| n).unwrap_or_else(|| operator_id.to_string()),
                expires_at,
            })
        }
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            // Read the holder so the caller can surface a useful error.
            let row: Option<(String, String, i64)> = sqlx::query_as(
                "SELECT l.operator_id, COALESCE(o.name, l.operator_id), l.expires_at \
                 FROM action_locks l \
                 LEFT JOIN operators o ON o.id = l.operator_id \
                 WHERE l.lock_key = ?",
            )
            .bind(lock_key)
            .fetch_optional(pool)
            .await
            .map_err(AcquireError::Sql)?;
            let conflict = match row {
                Some((op_id, name, exp)) => LockConflict {
                    lock_key: lock_key.to_string(),
                    operator_id: op_id,
                    operator_name: name,
                    expires_at: exp,
                },
                // Race: the holder dropped the lock between our INSERT
                // failure and the SELECT. Treat as conflict-with-empty
                // so the caller still returns an error and the operator
                // retries.
                None => LockConflict {
                    lock_key: lock_key.to_string(),
                    operator_id: "?".into(),
                    operator_name: "?".into(),
                    expires_at: 0,
                },
            };
            Err(AcquireError::Conflict(conflict))
        }
        Err(e) => Err(AcquireError::Sql(e)),
    }
}

/// Drop a lock on success. Idempotent — if the lock has already
/// expired and been GC'd, returns `Ok(false)`.
pub async fn release(pool: &SqlitePool, lock_key: &str) -> Result<bool, sqlx::Error> {
    let res = sqlx::query("DELETE FROM action_locks WHERE lock_key = ?")
        .bind(lock_key)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Sweep expired rows. Returns the list of lock_keys that were
/// dropped so the caller can emit unlock broadcasts.
pub async fn gc_expired(pool: &SqlitePool, now: i64) -> Result<Vec<String>, sqlx::Error> {
    let stale: Vec<(String,)> =
        sqlx::query_as("SELECT lock_key FROM action_locks WHERE expires_at < ?")
            .bind(now)
            .fetch_all(pool)
            .await?;
    if stale.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query("DELETE FROM action_locks WHERE expires_at < ?")
        .bind(now)
        .execute(pool)
        .await?;
    Ok(stale.into_iter().map(|(k,)| k).collect())
}

/// Read all currently-held locks (for snapshot inclusion). Drops
/// expired entries on read — keeps the snapshot consistent even if
/// the GC tick hasn't fired yet.
pub async fn list_active(pool: &SqlitePool, now: i64) -> Result<Vec<LockInfo>, sqlx::Error> {
    let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
        "SELECT l.lock_key, l.operator_id, COALESCE(o.name, l.operator_id), l.expires_at \
         FROM action_locks l \
         LEFT JOIN operators o ON o.id = l.operator_id \
         WHERE l.expires_at >= ?",
    )
    .bind(now)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(lock_key, operator_id, operator_name, expires_at)| LockInfo {
            lock_key,
            operator_id,
            operator_name,
            expires_at,
        })
        .collect())
}
