//! P5 Theme F: SQLite hot-snapshot daemon.
//!
//! `VACUUM INTO` is SQLite's canonical online-backup primitive — it
//! copies the live database to a fresh file without blocking writers
//! (WAL mode keeps readers consistent against the copy point). The
//! resulting file is a self-contained, defragmented snapshot that
//! the restore script can swap into place after stopping the world
//! process.
//!
//! Wiring: `spawn_backup_tick` reads three env vars at boot and either
//! starts a periodic Tokio task or returns silently:
//!
//!   CLIPTOWN_BACKUP_DIR             — required. Where snapshots land.
//!   CLIPTOWN_BACKUP_INTERVAL_HOURS  — default 6. Tick period.
//!   CLIPTOWN_BACKUP_KEEP            — default 14. Retention count.
//!
//! Each snapshot is named `cliptown-YYYYMMDD-HHMMSS.db`. After each
//! write we sweep the dir and prune oldest-first beyond KEEP. A
//! corrupt snapshot is logged but doesn't abort the daemon — the
//! next interval retries.

use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default snapshot frequency in hours when the env var is unset.
pub const DEFAULT_INTERVAL_HOURS: u64 = 6;
/// Default retention count when the env var is unset.
pub const DEFAULT_KEEP: usize = 14;

#[derive(Debug)]
pub enum BackupError {
    UnsafePath(String),
    Io(std::io::Error),
    Sql(sqlx::Error),
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackupError::UnsafePath(p) => write!(f, "unsafe path: {p}"),
            BackupError::Io(e) => write!(f, "io: {e}"),
            BackupError::Sql(e) => write!(f, "sql: {e}"),
        }
    }
}

/// Write a hot snapshot to `dest`. Refuses paths containing single
/// quotes or backslashes — they'd let an attacker who controls the
/// dir env var escape the SQL string we splice into.
pub async fn snapshot_to(pool: &SqlitePool, dest: &Path) -> Result<(), BackupError> {
    let dest_str = dest
        .to_str()
        .ok_or_else(|| BackupError::UnsafePath(dest.display().to_string()))?;
    if dest_str.contains('\'') || dest_str.contains('\\') {
        return Err(BackupError::UnsafePath(dest_str.to_string()));
    }
    // Ensure parent dir exists; VACUUM INTO won't create it.
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(BackupError::Io)?;
    }
    // `VACUUM INTO` doesn't accept parameter binding — splice the
    // path inline. The character whitelist above bounds the splice.
    let q = format!("VACUUM INTO '{}'", dest_str);
    sqlx::query(&q).execute(pool).await.map_err(BackupError::Sql)?;
    Ok(())
}

/// Build the timestamped snapshot filename inside `dir`.
pub fn snapshot_filename(dir: &Path, now: chrono::DateTime<chrono::Utc>) -> PathBuf {
    dir.join(format!("cliptown-{}.db", now.format("%Y%m%d-%H%M%S")))
}

/// Sweep `dir` and delete oldest snapshots beyond `keep`. Returns
/// the count actually deleted. Non-snapshot files are ignored.
pub fn prune(dir: &Path, keep: usize) -> Result<usize, BackupError> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(BackupError::Io(e)),
    };
    let mut snaps: Vec<PathBuf> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        let fname = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        if fname.starts_with("cliptown-") && fname.ends_with(".db") {
            snaps.push(path);
        }
    }
    // Sort newest-first by filename (timestamp is monotonic in the
    // canonical format) so we can delete the tail.
    snaps.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    if snaps.len() <= keep {
        return Ok(0);
    }
    let mut deleted = 0;
    for path in snaps.iter().skip(keep) {
        if std::fs::remove_file(path).is_ok() {
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// Read the three env vars and spawn the periodic snapshot task. No-op
/// when `CLIPTOWN_BACKUP_DIR` is unset. Called from
/// `loop_::spawn_with_layout` alongside the other GC ticks.
pub fn spawn_backup_tick(pool: SqlitePool) {
    let dir = match std::env::var("CLIPTOWN_BACKUP_DIR") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => return,
    };
    let interval_hours = std::env::var("CLIPTOWN_BACKUP_INTERVAL_HOURS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(DEFAULT_INTERVAL_HOURS);
    let keep = std::env::var("CLIPTOWN_BACKUP_KEEP")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(DEFAULT_KEEP);
    tracing::info!(
        component = "backup",
        dir = %dir.display(),
        interval_hours,
        keep,
        "backup tick enabled"
    );
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_hours * 3600));
        // First tick fires immediately so a freshly-booted world has
        // a snapshot within seconds rather than `interval_hours`.
        loop {
            interval.tick().await;
            let now = chrono::Utc::now();
            let dest = snapshot_filename(&dir, now);
            match snapshot_to(&pool, &dest).await {
                Ok(()) => {
                    tracing::info!(component = "backup", path = %dest.display(), "snapshot ok");
                    match prune(&dir, keep) {
                        Ok(n) if n > 0 => tracing::info!(component = "backup", pruned = n, "rotated"),
                        Ok(_) => {}
                        Err(e) => tracing::warn!(component = "backup", err = %e, "prune failed"),
                    }
                }
                Err(e) => {
                    tracing::warn!(component = "backup", err = %e, dest = %dest.display(),
                        "snapshot failed; will retry next tick");
                }
            }
        }
    });
}
