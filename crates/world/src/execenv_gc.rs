//! P3 carry-forward: world-side periodic execenv GC.
//!
//! `scripts/gc-execenv.sh` is the operator-run cousin of this; for unattended
//! deployments we want a daemon that runs the same selection criteria
//! automatically every N hours. Opt-in via `CLIPTOWN_EXECENV_GC_ENABLED=1`
//! to keep dev / smoke environments unaffected.
//!
//! Selection: tasks in terminal states (`done` | `failed` | `escalated`)
//! whose `updated_at` is older than `max_age_secs`. Removal path:
//! `<workspaces_root>/<startup_id>/<task_id>/` — same shape the worker
//! `prepareWorkdir` creates. Artifacts (`<workspaces_root>/<startup_id>/artifacts/`)
//! are NEVER touched; they're addressed by task id but lie under a sibling
//! directory.

use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct GcConfig {
    pub workspaces_root: PathBuf,
    pub max_age_secs: i64,
    pub interval: Duration,
}

impl GcConfig {
    /// Read config from env vars. None if `CLIPTOWN_EXECENV_GC_ENABLED` ≠ "1".
    /// Defaults: 7 day age, 6 hour interval, `./workspaces` root.
    pub fn from_env() -> Option<Self> {
        if std::env::var("CLIPTOWN_EXECENV_GC_ENABLED").as_deref() != Ok("1") {
            return None;
        }
        let days: i64 = std::env::var("CLIPTOWN_EXECENV_GC_AGE_DAYS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(7);
        let interval_hours: u64 = std::env::var("CLIPTOWN_EXECENV_GC_INTERVAL_HOURS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(6);
        let workspaces_root = std::env::var("CLIPTOWN_WORKSPACES_ROOT")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./workspaces"));
        Some(Self {
            workspaces_root,
            max_age_secs: days * 86_400,
            interval: Duration::from_secs(interval_hours * 3600),
        })
    }
}

/// Spawn the periodic GC task. The handle is detached — cancellation happens
/// at process exit (cliptown's world process has no other lifetime).
pub fn spawn(pool: SqlitePool, cfg: GcConfig) {
    let GcConfig { workspaces_root, max_age_secs, interval } = cfg;
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // Skip the immediate first tick so we don't reap on cold boot — give
        // the world a few seconds to finish migrations + accept connections.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let result = run_pass(&pool, &workspaces_root, max_age_secs).await;
            match result {
                Ok(reaped) => {
                    if reaped > 0 {
                        tracing::info!(
                            component = "execenv_gc",
                            event = "pass_complete",
                            reaped,
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(component = "execenv_gc", err = %e, "pass failed");
                }
            }
        }
    });
}

/// Run one GC pass. Returns the number of workdirs removed. Visible to tests
/// so they can drive a single pass without spinning the timer.
pub async fn run_pass(
    pool: &SqlitePool,
    workspaces_root: &Path,
    max_age_secs: i64,
) -> Result<usize, sqlx::Error> {
    let cutoff = chrono::Utc::now().timestamp() - max_age_secs;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT startup_id, id FROM tasks \
         WHERE status IN ('done', 'failed', 'escalated') AND updated_at < ?",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    let mut reaped = 0usize;
    for (startup_id, task_id) in rows {
        let target = workspaces_root.join(&startup_id).join(&task_id);
        if !target.exists() {
            continue;
        }
        if let Err(e) = tokio::fs::remove_dir_all(&target).await {
            tracing::warn!(
                component = "execenv_gc",
                target = %target.display(),
                err = %e,
                "remove_dir_all failed; skipping",
            );
            continue;
        }
        reaped += 1;
    }
    Ok(reaped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage;
    use std::sync::{Mutex, OnceLock};

    /// Serialize the env-mutating tests so they don't race under
    /// `cargo test`'s default thread pool. Same pattern as
    /// `tests/agent_supervisor.rs::env_lock`.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    async fn fresh_pool() -> SqlitePool {
        let dir = tempfile::tempdir().unwrap();
        let pool = storage::open(dir.path().join("t.db").to_str().unwrap())
            .await
            .unwrap();
        crate::seed::seed_if_empty(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
             VALUES ('s1', 'a', 'g', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
        ).execute(&pool).await.unwrap();
        std::mem::forget(dir);
        pool
    }

    fn seed_workdir(root: &Path, sid: &str, tid: &str) {
        let p = root.join(sid).join(tid);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("artifact.md"), "x").unwrap();
    }

    #[tokio::test]
    async fn reaps_only_terminal_old_tasks() {
        let pool = fresh_pool().await;
        let ws = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().timestamp();
        // T1: done, 30 days old → reap.
        sqlx::query(
            "INSERT INTO tasks (id, startup_id, title, description, status, created_at, updated_at) \
             VALUES ('T1', 's1', 't', 'd', 'done', ?, ?)",
        ).bind(now - 30 * 86400).bind(now - 30 * 86400).execute(&pool).await.unwrap();
        seed_workdir(ws.path(), "s1", "T1");
        // T2: in_progress, 30 days old → keep.
        sqlx::query(
            "INSERT INTO tasks (id, startup_id, title, description, status, created_at, updated_at) \
             VALUES ('T2', 's1', 't', 'd', 'in_progress', ?, ?)",
        ).bind(now - 30 * 86400).bind(now - 30 * 86400).execute(&pool).await.unwrap();
        seed_workdir(ws.path(), "s1", "T2");
        // T3: done, fresh → keep.
        sqlx::query(
            "INSERT INTO tasks (id, startup_id, title, description, status, created_at, updated_at) \
             VALUES ('T3', 's1', 't', 'd', 'done', ?, ?)",
        ).bind(now - 60).bind(now - 60).execute(&pool).await.unwrap();
        seed_workdir(ws.path(), "s1", "T3");

        let reaped = run_pass(&pool, ws.path(), 7 * 86400).await.unwrap();
        assert_eq!(reaped, 1);
        assert!(!ws.path().join("s1").join("T1").exists());
        assert!(ws.path().join("s1").join("T2").exists());
        assert!(ws.path().join("s1").join("T3").exists());
    }

    #[tokio::test]
    async fn missing_workdir_is_silently_skipped() {
        let pool = fresh_pool().await;
        let ws = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO tasks (id, startup_id, title, description, status, created_at, updated_at) \
             VALUES ('T_ghost', 's1', 't', 'd', 'done', ?, ?)",
        ).bind(now - 30 * 86400).bind(now - 30 * 86400).execute(&pool).await.unwrap();
        // No workdir on disk for T_ghost — pass should succeed with reaped=0.
        let reaped = run_pass(&pool, ws.path(), 7 * 86400).await.unwrap();
        assert_eq!(reaped, 0);
    }

    #[test]
    fn from_env_disabled_by_default() {
        let _g = env_lock();
        std::env::remove_var("CLIPTOWN_EXECENV_GC_ENABLED");
        assert!(GcConfig::from_env().is_none());
    }

    #[test]
    fn from_env_reads_overrides() {
        let _g = env_lock();
        std::env::set_var("CLIPTOWN_EXECENV_GC_ENABLED", "1");
        std::env::set_var("CLIPTOWN_EXECENV_GC_AGE_DAYS", "3");
        std::env::set_var("CLIPTOWN_EXECENV_GC_INTERVAL_HOURS", "2");
        std::env::set_var("CLIPTOWN_WORKSPACES_ROOT", "/tmp/cliptown-test");
        let cfg = GcConfig::from_env().expect("should be enabled");
        assert_eq!(cfg.max_age_secs, 3 * 86400);
        assert_eq!(cfg.interval, Duration::from_secs(2 * 3600));
        assert_eq!(cfg.workspaces_root, PathBuf::from("/tmp/cliptown-test"));
        // Clean up so other tests don't see these.
        std::env::remove_var("CLIPTOWN_EXECENV_GC_ENABLED");
        std::env::remove_var("CLIPTOWN_EXECENV_GC_AGE_DAYS");
        std::env::remove_var("CLIPTOWN_EXECENV_GC_INTERVAL_HOURS");
        std::env::remove_var("CLIPTOWN_WORKSPACES_ROOT");
    }
}
