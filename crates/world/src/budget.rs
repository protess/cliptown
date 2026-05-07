//! Budget enforcement (M1.15): cost computation, threshold tripping, and
//! pause-all. Phase 0 ships a hardcoded pricing table — production will read
//! from a config. The contract here is:
//!
//! 1. `apply_report` is called from `cmd_worker::dispatch` on every
//!    `WorkerInbound::ReportBudget`. It computes `cost_usd`, increments
//!    `startups.budget_spent_usd`, appends a `budget_events` row, and reports
//!    which threshold (if any) was newly crossed by this report.
//! 2. The caller fans out side-effects:
//!    - Any threshold crossing => `record_threshold_event` (severity-tagged
//!      row in `system_events` for the console event feed).
//!    - 80%: warn (already covered by the system_event row).
//!    - 95%: scheduler skips new dispatches; no extra signal needed here.
//!    - 100%: caller invokes `pause_startup` to broadcast a `Pause` frame to
//!      every worker of the affected startup.
//! 3. Auto-resume after PATCH `/api/startups/:id { budget_cap_usd: N }` is
//!    implicit: `newly_crossed` only fires on transitions, so raising the cap
//!    above current spend prevents future reports from re-tripping 100%.

use crate::persist;
use crate::state::WorldView;
use serde_json::json;
use sqlx::SqlitePool;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Reason code embedded in `WorkerOutbound::Pause`. Phase 0 only emits
/// `budget_exhausted` (from the 100% threshold); future phases may add other
/// codes (e.g. `dissolving`).
const PAUSE_REASON_BUDGET: &str = "budget_exhausted";

/// model_id → ($/Mtok input, $/Mtok output). Phase 0 placeholders sourced from
/// anthropic.com/pricing & openai.com/api/pricing as of 2024-vintage data.
/// Re-verify against current vendor pricing before any non-zero-cost run.
pub fn price_per_mtok(model_id: &str) -> Option<(f64, f64)> {
    match model_id {
        // Anthropic
        "claude-3-5-sonnet" | "claude-3-5-sonnet-20241022" => Some((3.00, 15.00)),
        "claude-3-5-haiku" | "claude-3-5-haiku-20241022" => Some((1.00, 5.00)),
        "claude-3-opus" | "claude-3-opus-20240229" => Some((15.00, 75.00)),
        // OpenAI (Codex)
        "gpt-4o" | "gpt-4o-2024-08-06" => Some((2.50, 10.00)),
        "gpt-4o-mini" => Some((0.15, 0.60)),
        // OpenCode default
        "opencode-default" => Some((1.00, 5.00)),
        _ => None,
    }
}

/// Compute USD cost for a single report. Unknown models price at $0 (Phase 0
/// placeholder); since cost is $0, no spend increment occurs and no threshold
/// transition or `system_events` row is emitted. We log a warning so unknown
/// models surface in operator logs rather than getting silently zero-billed.
pub fn cost_usd(model_id: &str, in_tokens: u64, out_tokens: u64) -> f64 {
    let (in_p, out_p) = match price_per_mtok(model_id) {
        Some(p) => p,
        None => {
            tracing::warn!(model_id = %model_id, "unknown model pricing; treating as $0");
            (0.0, 0.0)
        }
    };
    (in_tokens as f64 / 1_000_000.0) * in_p + (out_tokens as f64 / 1_000_000.0) * out_p
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Threshold {
    Warn80,
    Warn95,
    Pause100,
}

/// Determine which threshold (if any) is newly crossed by this report.
/// Compares `prev_spent / cap` with `new_spent / cap`. Returns the highest
/// threshold crossed in this single transition (so a report that vaults from
/// 70% to 110% reports `Pause100` and the caller pauses).
pub fn newly_crossed(prev_spent: f64, new_spent: f64, cap: f64) -> Option<Threshold> {
    if cap <= 0.0 {
        return None;
    }
    let was = prev_spent / cap;
    let now = new_spent / cap;
    if was < 1.00 && now >= 1.00 {
        return Some(Threshold::Pause100);
    }
    if was < 0.95 && now >= 0.95 {
        return Some(Threshold::Warn95);
    }
    if was < 0.80 && now >= 0.80 {
        return Some(Threshold::Warn80);
    }
    None
}

/// Apply a budget report from a worker. Reads the previous spend + cap from
/// `startups`, increments `budget_spent_usd`, appends a `budget_events` row,
/// and returns `(new_spent, cap, threshold_crossed)` so the caller can
/// dispatch side-effects.
///
/// `agent_id` and `task_id` are persisted with the budget event so post-mortem
/// replay can attribute spend to the originating worker/task.
pub async fn apply_report(
    pool: &SqlitePool,
    startup_id: &str,
    agent_id: &str,
    task_id: Option<&str>,
    model_id: &str,
    in_tokens: u64,
    out_tokens: u64,
) -> Result<(f64, f64, Option<Threshold>), sqlx::Error> {
    let cost = cost_usd(model_id, in_tokens, out_tokens);

    let row: Option<(f64, f64)> = sqlx::query_as(
        "SELECT budget_spent_usd, budget_cap_usd FROM startups WHERE id = ?",
    )
    .bind(startup_id)
    .fetch_optional(pool)
    .await?;
    let (prev_spent, cap) = match row {
        Some(r) => r,
        None => return Ok((0.0, 0.0, None)),
    };
    let new_spent = prev_spent + cost;

    sqlx::query("UPDATE startups SET budget_spent_usd = ? WHERE id = ?")
        .bind(new_spent)
        .bind(startup_id)
        .execute(pool)
        .await?;

    // Persist via the shared helper (it owns the column ordering + ts) so the
    // schema usage stays in `persist::` and we don't drift from M1.10.
    if let Err(e) = persist::record_budget_event(
        pool,
        startup_id,
        agent_id,
        task_id,
        in_tokens as i64,
        out_tokens as i64,
        cost,
        model_id,
    )
    .await
    {
        // Map anyhow → sqlx::Error path is awkward; surface as RowNotFound is
        // wrong, so log and continue. The spend update above already committed.
        tracing::warn!(error = %e, startup_id = %startup_id, "record_budget_event failed");
    }

    let threshold = newly_crossed(prev_spent, new_spent, cap);
    Ok((new_spent, cap, threshold))
}

/// Send a Pause frame to every worker of `startup_id`. Used when the 100%
/// threshold trips. Returns the count of pause frames pushed (= number of
/// connected workers for this startup).
pub fn pause_startup(
    world: &WorldView,
    out_bus: &HashMap<String, mpsc::Sender<serde_json::Value>>,
    startup_id: &str,
) -> usize {
    let payload = serde_json::to_value(&crate::protocol::WorkerOutbound::Pause {
        v: 1,
        reason: PAUSE_REASON_BUDGET.to_string(),
    })
    .unwrap_or_else(|_| json!({}));
    let mut sent = 0;
    for (agent_id, avatar) in &world.avatars {
        if avatar.startup_id != startup_id {
            continue;
        }
        if let Some(tx) = out_bus.get(agent_id) {
            if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) =
                tx.try_send(payload.clone())
            {
                tracing::warn!(agent_id = %agent_id, "out_bus full, dropping pause");
            }
            sent += 1;
        }
    }
    sent
}

/// Send a `BudgetWarning` frame to every worker of `startup_id`. Mirrors
/// `pause_startup` but for the 80% / 95% thresholds. Spec §6.1 requires this
/// signal so workers can throttle / wrap-up before the 100% pause hits.
pub fn warn_startup(
    world: &WorldView,
    out_bus: &HashMap<String, mpsc::Sender<serde_json::Value>>,
    startup_id: &str,
    remaining_usd: f64,
    percent_used: u32,
) -> usize {
    let payload = serde_json::to_value(&crate::protocol::WorkerOutbound::BudgetWarning {
        v: 1,
        remaining_usd,
        percent_used,
    })
    .unwrap_or_else(|_| json!({}));
    let mut sent = 0;
    for (agent_id, avatar) in &world.avatars {
        if avatar.startup_id != startup_id {
            continue;
        }
        if let Some(tx) = out_bus.get(agent_id) {
            if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) =
                tx.try_send(payload.clone())
            {
                tracing::warn!(agent_id = %agent_id, "out_bus full, dropping budget_warning");
            }
            sent += 1;
        }
    }
    sent
}

/// Record a `system_events` row for a tripped threshold. 80/95 fire as
/// `severity=warn`; 100 fires as `severity=alert`. Console event feed (M2)
/// will surface these as toasts.
pub async fn record_threshold_event(
    pool: &SqlitePool,
    startup_id: &str,
    threshold: Threshold,
    spent: f64,
    cap: f64,
) -> Result<(), anyhow::Error> {
    let (severity, kind) = match threshold {
        Threshold::Warn80 => ("warn", "budget_warn_80"),
        Threshold::Warn95 => ("warn", "budget_warn_95"),
        Threshold::Pause100 => ("alert", "budget_pause_100"),
    };
    let payload = json!({
        "startup_id": startup_id,
        "spent_usd": spent,
        "cap_usd": cap,
    })
    .to_string();
    persist::record_system_event(pool, Some(startup_id), kind, &payload, severity).await
}
