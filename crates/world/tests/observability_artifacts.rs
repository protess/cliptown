//! P5 Theme D: keep the observability artifacts honest.
//!
//! - The Grafana dashboard JSON must parse and contain panels.
//! - The Alertmanager rule YAML must parse and contain at least
//!   one rule under the cliptown group.
//! - Every Prometheus expression in the alert rules must reference
//!   a metric name we actually export from `crates/world/src/metrics.rs`.
//!
//! Cheap to run; catches typos before they land in prod ops surfaces.

use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points at crates/world; walk up twice.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

#[test]
fn grafana_dashboard_parses_and_has_panels() {
    let path = repo_root().join("docs/observability/grafana/cliptown-overview.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let v: Value = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    let panels = v["panels"].as_array().expect("panels array");
    assert!(
        !panels.is_empty(),
        "dashboard must declare at least one panel"
    );
    // Every panel must have an `id`, `title`, and at least one
    // target. A typo'd panel with no targets renders an empty box —
    // pin the contract.
    for p in panels {
        assert!(p["id"].is_number(), "panel missing id: {p}");
        assert!(p["title"].is_string(), "panel missing title: {p}");
        let targets = p["targets"].as_array().expect("panel.targets");
        assert!(!targets.is_empty(), "panel {} has no targets", p["title"]);
    }
}

#[test]
fn alert_rules_parse_and_reference_known_metrics() {
    let path = repo_root().join("docs/observability/alerts/cliptown.yml");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let doc: serde_yaml::Value = serde_yaml::from_str(&raw)
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    let groups = doc["groups"].as_sequence().expect("groups sequence");
    assert!(!groups.is_empty(), "must declare at least one rule group");

    // Metrics we actually export from crates/world/src/metrics.rs.
    // Keep in sync — adding a metric also enables alerting on it.
    let known: HashSet<&str> = [
        "cliptown_mcp_calls_total",
        "cliptown_mcp_errors_total",
        "cliptown_agents",
        "cliptown_startups_active",
        "cliptown_budget_spent_usd",
        "cliptown_budget_cap_usd",
        "cliptown_tasks",
        "cliptown_tick_seq",
    ]
    .into_iter()
    .collect();

    let mut rule_count = 0;
    for g in groups {
        let rules = g["rules"].as_sequence().expect("rules sequence");
        for r in rules {
            rule_count += 1;
            let expr = r["expr"].as_str().expect("rule.expr");
            // Crude but effective: every cliptown_* substring in the
            // expression must appear in `known`. Splits on
            // non-identifier chars.
            for token in expr.split(|c: char| !c.is_alphanumeric() && c != '_') {
                if token.starts_with("cliptown_") {
                    assert!(
                        known.contains(token),
                        "rule references unknown metric {token} in expr `{expr}`"
                    );
                }
            }
            assert!(r["alert"].is_string(), "rule missing alert name: {r:?}");
            assert!(
                r["annotations"]["summary"].is_string(),
                "rule {} missing summary annotation",
                r["alert"].as_str().unwrap_or("?")
            );
        }
    }
    assert!(rule_count >= 3, "expected ≥3 alert rules; got {rule_count}");
}
