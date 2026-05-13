//! P3 Theme D: Prometheus-style /metrics endpoint.
//!
//! Hand-rolled text exposition format (no external prometheus crate
//! dependency). Counters are atomic; gauges are recomputed per scrape by
//! reading SQL + the in-memory WorldView. Cheap enough at cliptown's
//! current scale (single-startup-digit world); revisit if scrape latency
//! climbs past 100ms.

use sqlx::SqlitePool;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::health::Health;
use crate::state::WorldView;

/// Global counters, incremented from the MCP dispatcher.
pub struct Counters {
    pub mcp_calls_total: AtomicU64,
    pub mcp_errors_total: AtomicU64,
}

impl Counters {
    pub const fn new() -> Self {
        Self {
            mcp_calls_total: AtomicU64::new(0),
            mcp_errors_total: AtomicU64::new(0),
        }
    }

    pub fn inc_call(&self) {
        self.mcp_calls_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_error(&self) {
        self.mcp_errors_total.fetch_add(1, Ordering::Relaxed);
    }
}

/// Process-wide singleton. Read by the /metrics handler; written by
/// hot-path code via `crate::metrics::COUNTERS.inc_*()`.
pub static COUNTERS: Counters = Counters::new();

/// Render the Prometheus text exposition format for the current state.
/// Reads `view` for in-memory data + queries SQL for startup/task gauges.
pub async fn render(pool: &SqlitePool, view: &WorldView) -> String {
    let mut out = String::with_capacity(2048);

    let calls = COUNTERS.mcp_calls_total.load(Ordering::Relaxed);
    let errors = COUNTERS.mcp_errors_total.load(Ordering::Relaxed);
    let _ = writeln!(out, "# HELP cliptown_mcp_calls_total MCP tool call attempts (success + failure).");
    let _ = writeln!(out, "# TYPE cliptown_mcp_calls_total counter");
    let _ = writeln!(out, "cliptown_mcp_calls_total {calls}");
    let _ = writeln!(out, "# HELP cliptown_mcp_errors_total MCP tool calls that returned mcp_error.");
    let _ = writeln!(out, "# TYPE cliptown_mcp_errors_total counter");
    let _ = writeln!(out, "cliptown_mcp_errors_total {errors}");

    // Per-health-bucket agent counts. Counts the operator avatar too —
    // it's a real avatar by design.
    let mut bucket_counts: HashMap<&'static str, u64> = HashMap::new();
    for av in view.avatars.values() {
        let label = match av.health {
            Health::Online => "online",
            Health::RecentlyLost => "recently_lost",
            Health::Offline => "offline",
            Health::AboutToGc => "about_to_gc",
        };
        *bucket_counts.entry(label).or_insert(0) += 1;
    }
    let _ = writeln!(out, "# HELP cliptown_agents Avatar count by P2.1 health bucket.");
    let _ = writeln!(out, "# TYPE cliptown_agents gauge");
    for bucket in ["online", "recently_lost", "offline", "about_to_gc"] {
        let n = bucket_counts.get(bucket).copied().unwrap_or(0);
        let _ = writeln!(out, "cliptown_agents{{health=\"{bucket}\"}} {n}");
    }

    // Active startup count + per-startup budget.
    let startups: Vec<(String, String, f64, f64)> = sqlx::query_as(
        "SELECT id, name, budget_spent_usd, budget_cap_usd FROM startups WHERE status = 'active'",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let active_count = startups.len() as u64;
    let _ = writeln!(out, "# HELP cliptown_startups_active Currently active startups.");
    let _ = writeln!(out, "# TYPE cliptown_startups_active gauge");
    let _ = writeln!(out, "cliptown_startups_active {active_count}");

    let _ = writeln!(out, "# HELP cliptown_budget_spent_usd Per-startup spend.");
    let _ = writeln!(out, "# TYPE cliptown_budget_spent_usd gauge");
    let _ = writeln!(out, "# HELP cliptown_budget_cap_usd Per-startup cap.");
    let _ = writeln!(out, "# TYPE cliptown_budget_cap_usd gauge");
    for (id, _name, spent, cap) in &startups {
        let _ = writeln!(
            out,
            "cliptown_budget_spent_usd{{startup_id=\"{id}\"}} {spent}"
        );
        let _ = writeln!(
            out,
            "cliptown_budget_cap_usd{{startup_id=\"{id}\"}} {cap}"
        );
    }

    // Task count by status.
    let task_counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, COUNT(*) FROM tasks GROUP BY status",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let _ = writeln!(out, "# HELP cliptown_tasks Task count by status.");
    let _ = writeln!(out, "# TYPE cliptown_tasks gauge");
    for status in [
        "proposed",
        "queued",
        "in_progress",
        "awaiting_review",
        "changes_requested",
        "done",
        "failed",
        "escalated",
    ] {
        let n = task_counts
            .iter()
            .find(|(s, _)| s == status)
            .map(|(_, n)| *n)
            .unwrap_or(0);
        let _ = writeln!(out, "cliptown_tasks{{status=\"{status}\"}} {n}");
    }

    // Tick sequence — useful as a liveness signal independent of /health.
    let _ = writeln!(out, "# HELP cliptown_tick_seq Monotonic loop tick counter.");
    let _ = writeln!(out, "# TYPE cliptown_tick_seq counter");
    let _ = writeln!(out, "cliptown_tick_seq {}", view.tick_seq);

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage;

    #[tokio::test]
    async fn render_outputs_prometheus_format_with_zero_state() {
        let dir = tempfile::tempdir().unwrap();
        let pool = storage::open(dir.path().join("t.db").to_str().unwrap())
            .await
            .unwrap();
        crate::seed::seed_if_empty(&pool).await.unwrap();
        let view = WorldView::default();
        let out = render(&pool, &view).await;
        assert!(out.contains("# HELP cliptown_mcp_calls_total"));
        assert!(out.contains("cliptown_mcp_calls_total"));
        assert!(out.contains("cliptown_agents{health=\"online\"} 0"));
        assert!(out.contains("cliptown_startups_active 0"));
        assert!(out.contains("cliptown_tasks{status=\"proposed\"} 0"));
        assert!(out.contains("cliptown_tick_seq 0"));
    }

    #[tokio::test]
    async fn counters_increment_via_global() {
        let dir = tempfile::tempdir().unwrap();
        let pool = storage::open(dir.path().join("t.db").to_str().unwrap())
            .await
            .unwrap();
        crate::seed::seed_if_empty(&pool).await.unwrap();
        let view = WorldView::default();
        let before = COUNTERS.mcp_calls_total.load(Ordering::Relaxed);
        COUNTERS.inc_call();
        COUNTERS.inc_call();
        let out = render(&pool, &view).await;
        let after = COUNTERS.mcp_calls_total.load(Ordering::Relaxed);
        assert_eq!(after, before + 2);
        // Output reflects the new value.
        assert!(out.contains(&format!("cliptown_mcp_calls_total {after}")));
    }
}
