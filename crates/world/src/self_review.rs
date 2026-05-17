//! P6 Theme A: self-review check pipeline.
//!
//! Run by the agent before `task_done` (via the `self_review` MCP tool)
//! and optionally during `task_done` (via the `auto_check` arg, default
//! true). The pipeline returns a structured `must_fix` list with one
//! entry per failing check. `error` severity blocks `task_done`'s
//! status flip; `warn` severity just annotates the audit row.
//!
//! v1 pipeline:
//!   1. canonical_path — artifact_path matches
//!      `workspaces/<sid>/artifacts/<tid>.md` exactly. error.
//!   2. artifact_exists — file present and non-empty. error.
//!   3. json_lint — only when artifact extension is `.json`;
//!      `serde_json::from_str` succeeds. error.
//!   4. markdown_lint — placeholder (TS sidecar deferred); warn-
//!      severity "deferred" stub. Becomes real in P6.B.
//!   5. ts_rust_lint — placeholder for P6.B; warn-severity stub
//!      when the artifact extension matches.
//!
//! Each check returns a `CheckResult`. The aggregator returns
//! `SelfReviewOutcome` with `ok = (no error-severity failures)` and
//! the full `must_fix` list (warn + error combined). Callers decide
//! how to render that — `self_review` MCP tool puts it in the
//! response payload; `task_done` blocks on `ok == false`.

use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum Severity {
    Warn,
    Error,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Warn => "warn",
            Severity::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub check: &'static str,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SelfReviewOutcome {
    pub ok: bool,
    pub must_fix: Vec<CheckResult>,
}

impl SelfReviewOutcome {
    pub fn passed() -> Self {
        Self { ok: true, must_fix: Vec::new() }
    }
}

/// Build the canonical artifact path the agent must submit. Centralized
/// so the same string is used by both task_done's hard check and
/// self_review's soft check.
pub fn canonical_artifact_path(startup_id: &str, task_id: &str) -> String {
    format!("workspaces/{}/artifacts/{}.md", startup_id, task_id)
}

/// Run the v1 check pipeline. Always returns `Ok` — individual check
/// failures surface as entries in `must_fix`, not Rust errors. SQL/IO
/// errors that prevent a check from running surface as warn-severity
/// "could not run" entries.
pub async fn run(
    startup_id: &str,
    task_id: &str,
    artifact_path: &str,
) -> SelfReviewOutcome {
    let mut must_fix: Vec<CheckResult> = Vec::new();

    // 1. canonical_path
    let canonical = canonical_artifact_path(startup_id, task_id);
    if artifact_path != canonical {
        must_fix.push(CheckResult {
            check: "canonical_path",
            severity: Severity::Error,
            message: format!("expected {canonical}, got {artifact_path}"),
        });
        // No point running the rest — sandbox::resolve would refuse a
        // non-canonical path and the file-system probes would either
        // path-traverse or miss. Bail with a clean error.
        return SelfReviewOutcome { ok: false, must_fix };
    }

    // 2. artifact_exists — read the file via the same sandbox helper the
    // other tools use so a symlink-out doesn't slip past.
    let workspace_root = std::path::PathBuf::from(format!("workspaces/{startup_id}"));
    let inside = format!("artifacts/{task_id}.md");
    let resolved = match crate::sandbox::resolve(&workspace_root, &inside) {
        Ok(p) => p,
        Err(e) => {
            must_fix.push(CheckResult {
                check: "artifact_exists",
                severity: Severity::Error,
                message: format!("sandbox refused: {e}"),
            });
            return SelfReviewOutcome { ok: false, must_fix };
        }
    };
    let bytes = match tokio::fs::read(&resolved).await {
        Ok(b) => b,
        Err(e) => {
            must_fix.push(CheckResult {
                check: "artifact_exists",
                severity: Severity::Error,
                message: format!("read failed: {e}"),
            });
            return SelfReviewOutcome { ok: false, must_fix };
        }
    };
    if bytes.is_empty() {
        must_fix.push(CheckResult {
            check: "artifact_exists",
            severity: Severity::Error,
            message: "artifact is zero bytes".into(),
        });
        return SelfReviewOutcome { ok: false, must_fix };
    }

    // 3. json_lint — only if the canonical path was overridden to a JSON
    // artifact in a future schema; today canonical is always `.md` so
    // this branch is dead code but included for forward-compat when
    // P6.B's lint_artifact widens the gate.
    if Path::new(artifact_path)
        .extension()
        .and_then(|e| e.to_str())
        == Some("json")
    {
        match std::str::from_utf8(&bytes).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()) {
            Some(_) => {}
            None => must_fix.push(CheckResult {
                check: "json_lint",
                severity: Severity::Error,
                message: "artifact is not valid JSON".into(),
            }),
        }
    }

    // 4. markdown_lint — deferred until P6.B wires the TS sidecar.
    //    Warn severity so a missing linter doesn't block submission.
    if artifact_path.ends_with(".md") {
        must_fix.push(CheckResult {
            check: "markdown_lint",
            severity: Severity::Warn,
            message: "deferred: TS sidecar lint pipeline lands in P6.B".into(),
        });
    }

    let ok = !must_fix.iter().any(|c| c.severity == Severity::Error);
    SelfReviewOutcome { ok, must_fix }
}

/// Stamp `tasks.self_reviewed_at = now` on a passing run. Idempotent;
/// also writes an audit_trail entry capturing the full outcome so the
/// auto-recovery pass (P6.C) can see what's been tried.
pub async fn record(
    pool: &sqlx::SqlitePool,
    task_id: &str,
    agent_id: &str,
    outcome: &SelfReviewOutcome,
) -> Result<(), sqlx::Error> {
    if outcome.ok {
        sqlx::query("UPDATE tasks SET self_reviewed_at = unixepoch() WHERE id = ?")
            .bind(task_id)
            .execute(pool)
            .await?;
    }
    let _ = crate::persist::append_audit(
        pool,
        task_id,
        &serde_json::json!({
            "actor": "engineer",
            "kind": "self_review",
            "agent_id": agent_id,
            "outcome": if outcome.ok { "pass" } else { "fail" },
            "checks": outcome.must_fix.iter().map(|c| serde_json::json!({
                "check": c.check,
                "severity": c.severity.as_str(),
                "message": c.message,
            })).collect::<Vec<_>>(),
        })
        .to_string(),
    )
    .await;
    Ok(())
}
