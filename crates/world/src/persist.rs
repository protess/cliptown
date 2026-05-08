//! Persist helpers — typed entry points for the world's command dispatch to write
//! audit_trail, epistemic_log, fs_audit, budget_events, system_events, and position
//! snapshots into SQLite. All writes go through this module so the schema usage
//! stays in one place.

use anyhow::Result;
use sqlx::SqlitePool;

/// Append a JSON object (already serialized) to a task's `audit_trail` JSON array.
/// Used for ops events: task_assigned, move_intent, tool_call_pre/post, etc.
pub async fn append_audit(pool: &SqlitePool, task_id: &str, entry_json: &str) -> Result<()> {
    sqlx::query(
        "UPDATE tasks SET audit_trail = json_insert(audit_trail, '$[#]', json(?)), updated_at = unixepoch() WHERE id = ?"
    )
    .bind(entry_json)
    .bind(task_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Append a reasoning entry to a task's `epistemic_log` JSON array.
/// Used for hypothesis_state, test_record, hypothesis_resolve.
pub async fn append_epistemic(pool: &SqlitePool, task_id: &str, entry_json: &str) -> Result<()> {
    sqlx::query(
        "UPDATE tasks SET epistemic_log = json_insert(epistemic_log, '$[#]', json(?)), updated_at = unixepoch() WHERE id = ?"
    )
    .bind(entry_json)
    .bind(task_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Insert a row into `fs_audit`. Every CLI file operation produces one row.
pub async fn record_fs_audit(
    pool: &SqlitePool,
    startup_id: &str,
    agent_id: &str,
    op: &str,
    path: &str,
    bytes: i64,
    ok: bool,
    err: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO fs_audit (id, startup_id, agent_id, op, path, bytes, ok, error, ts) VALUES (?, ?, ?, ?, ?, ?, ?, ?, unixepoch())"
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(startup_id)
    .bind(agent_id)
    .bind(op)
    .bind(path)
    .bind(bytes)
    .bind(if ok { 1 } else { 0 })
    .bind(err)
    .execute(pool)
    .await?;
    Ok(())
}

/// Insert a row into `budget_events`. Used by the world's budget enforcer (M1.15).
pub async fn record_budget_event(
    pool: &SqlitePool,
    startup_id: &str,
    agent_id: &str,
    task_id: Option<&str>,
    in_tokens: i64,
    out_tokens: i64,
    cost_usd: f64,
    model_id: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO budget_events (id, startup_id, agent_id, task_id, in_tokens, out_tokens, cost_usd, model_id, ts) VALUES (?, ?, ?, ?, ?, ?, ?, ?, unixepoch())"
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(startup_id)
    .bind(agent_id)
    .bind(task_id)
    .bind(in_tokens)
    .bind(out_tokens)
    .bind(cost_usd)
    .bind(model_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// DEPRECATED for new callers — prefer `crate::emit::emit_system_event`,
/// which broadcasts a `ConsoleOutbound::SystemEvent` frame to operator
/// consoles in addition to writing the SQL row. Existing callers may keep
/// using this for SQL-only persistence; new callers must migrate.
///
/// Insert a row into `system_events`. Used for severity-tiered surfacing (info/warn/alert/critical).
pub async fn record_system_event(
    pool: &SqlitePool,
    startup_id: Option<&str>,
    kind: &str,
    payload_json: &str,
    severity: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO system_events (id, startup_id, kind, payload, severity, ts) VALUES (?, ?, ?, ?, ?, unixepoch())"
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(startup_id)
    .bind(kind)
    .bind(payload_json)
    .bind(severity)
    .execute(pool)
    .await?;
    Ok(())
}

/// Snapshot avatar positions into `agents.position_json`. Called every 60 ticks
/// by the world loop to bound crash rewind to ≤ 60s. Caller passes a slice of
/// (agent_id, position_json_string) pairs; this fn batches the UPDATE.
pub async fn snapshot_positions(pool: &SqlitePool, avatars: &[(String, String)]) -> Result<()> {
    if avatars.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for (agent_id, position_json) in avatars {
        sqlx::query("UPDATE agents SET position_json = ? WHERE id = ?")
            .bind(position_json)
            .bind(agent_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
