//! Console-event emission helper. Wraps the SQL persist of system_events and
//! broadcasts a matching ConsoleOutbound::SystemEvent frame. Callers should
//! prefer this over `persist::record_system_event` (which only writes SQL and
//! doesn't reach the operator console).
//!
//! The helper owns `id` and `ts` — both fields are generated in Rust and bound
//! to the SQL INSERT, so the broadcast frame and the persisted row carry
//! identical values. Wire-format `ts` is UNIX milliseconds (frontend renders
//! `new Date(m.ts)`); SQL stores seconds for compatibility with existing
//! `unixepoch()`-based queries.

use crate::protocol::ConsoleOutbound;
use serde_json::Value;
use sqlx::SqlitePool;
use tokio::sync::broadcast;

pub async fn emit_system_event(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<ConsoleOutbound>,
    startup_id: Option<&str>,
    kind: &str,
    payload: &str,
    severity: &str,
) -> Result<(), sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let ts_secs = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO system_events (id, startup_id, kind, payload, severity, ts) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(startup_id)
    .bind(kind)
    .bind(payload)
    .bind(severity)
    .bind(ts_secs)
    .execute(pool)
    .await?;

    // Parse the payload into a JSON value for the broadcast frame. The SQL
    // row above stores the raw string regardless — when parsing fails we MUST
    // log loudly and surface the raw string on the wire too, otherwise the
    // SQL audit log and the operator console see different data (the prior
    // `unwrap_or(Value::Null)` silently degraded to `payload: null` while
    // SQL kept the malformed string). Loud-fail + preserve-the-raw lets
    // operators notice the producer bug instead of squinting at quiet nulls.
    let payload_value = match serde_json::from_str::<Value>(payload) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(
                component = "emit",
                kind = %kind,
                error = %e,
                payload_preview = %payload.chars().take(120).collect::<String>(),
                "emit_system_event payload is not valid JSON; broadcasting raw string instead",
            );
            Value::String(payload.to_string())
        }
    };

    // `let _` discards the Result — Err means zero subscribers, not a failure.
    let _ = event_tx.send(ConsoleOutbound::SystemEvent {
        v: 1,
        severity: severity.into(),
        kind: kind.into(),
        startup_id: startup_id.map(String::from),
        payload: payload_value,
        ts: ts_secs * 1000, // milliseconds on the wire
    });
    Ok(())
}
