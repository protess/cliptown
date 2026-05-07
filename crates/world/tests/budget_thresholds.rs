//! Tests for M1.15 budget enforcement: pricing, threshold detection,
//! `apply_report` side-effects (spend update + budget_events row), pause-all
//! broadcast on the 100% threshold, and system_events row on each crossing.

use cliptown_world::{
    budget,
    state::{AvatarView, WorldView},
    storage,
};
use std::collections::HashMap;
use tokio::sync::mpsc;

async fn fixture() -> (WorldView, sqlx::SqlitePool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("budget-test.db");
    let pool = storage::open(p.to_str().unwrap()).await.unwrap();

    // Seed a town directly so we don't depend on `seed_if_empty` (which would
    // also seed rooms/doors we don't need here).
    sqlx::query(
        "INSERT INTO towns (id, name, map_json) VALUES ('town_default', 'T', '{}')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, budget_spent_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'alpha', 'goal', 10.0, 0.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
    )
    .execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, status) \
         VALUES ('a1', 's1', 'A1', 'engineer', 'claude_code', 'claude-3-5-sonnet', '{}', 'suite_1', 'idle')"
    )
    .execute(&pool).await.unwrap();

    let mut w = WorldView::default();
    w.avatars.insert(
        "a1".to_string(),
        AvatarView {
            agent_id: "a1".to_string(),
            startup_id: "s1".to_string(),
            role: "engineer".to_string(),
            backend: "claude_code".to_string(),
            current_pos: (3, 3),
            target_pos: None,
            room_id: "suite_1".to_string(),
            status: "idle".to_string(),
        },
    );
    (w, pool, dir)
}

#[test]
fn pricing_known_models() {
    assert!(budget::price_per_mtok("claude-3-5-sonnet").is_some());
    assert!(budget::price_per_mtok("claude-3-5-sonnet-20241022").is_some());
    assert!(budget::price_per_mtok("gpt-4o").is_some());
    assert!(budget::price_per_mtok("opencode-default").is_some());
    assert!(budget::price_per_mtok("totally-unknown").is_none());
}

#[test]
fn cost_calculation() {
    // sonnet: $3/Mtok in + $15/Mtok out → 1M in + 1M out = $18.
    let c = budget::cost_usd("claude-3-5-sonnet", 1_000_000, 1_000_000);
    assert!((c - 18.0).abs() < 1e-9, "expected $18, got {}", c);

    // Unknown model prices at $0 (caller can still see the report; the
    // matching system_events row keeps it from being silently ignored).
    let c = budget::cost_usd("unknown-model", 5_000_000, 5_000_000);
    assert_eq!(c, 0.0);
}

#[test]
fn newly_crossed_at_each_threshold() {
    use budget::Threshold::*;
    assert_eq!(budget::newly_crossed(0.0, 8.0, 10.0), Some(Warn80));
    assert_eq!(budget::newly_crossed(8.0, 9.5, 10.0), Some(Warn95));
    assert_eq!(budget::newly_crossed(9.5, 10.5, 10.0), Some(Pause100));
    // Already past 80%, not yet 95% → no new threshold.
    assert_eq!(budget::newly_crossed(8.0, 8.5, 10.0), None);
    // Far below 80% → none.
    assert_eq!(budget::newly_crossed(0.0, 0.5, 10.0), None);
    // Already past 100% → no re-trip.
    assert_eq!(budget::newly_crossed(10.5, 11.0, 10.0), None);
    // A single jumbo report can vault straight to 100% — return Pause100,
    // not Warn80, so the caller pauses immediately.
    assert_eq!(budget::newly_crossed(0.0, 11.0, 10.0), Some(Pause100));
    // cap = 0 disables enforcement (avoids div-by-zero).
    assert_eq!(budget::newly_crossed(0.0, 100.0, 0.0), None);
}

#[tokio::test]
async fn apply_report_increments_spend_and_logs_event() {
    let (_w, pool, _tmp) = fixture().await;
    let (spent, cap, threshold) = budget::apply_report(
        &pool,
        "s1",
        "a1",
        None,
        "claude-3-5-sonnet",
        100_000,
        100_000,
    )
    .await
    .unwrap();
    // 100k in + 100k out @ $3/$15 per Mtok = $0.30 + $1.50 = $1.80.
    assert!((spent - 1.80).abs() < 1e-9, "expected spend $1.80, got {}", spent);
    assert_eq!(cap, 10.0);
    assert!(threshold.is_none(), "18% should not trip any threshold");

    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM budget_events WHERE startup_id='s1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1);

    let stored: (f64,) = sqlx::query_as("SELECT budget_spent_usd FROM startups WHERE id='s1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!((stored.0 - 1.80).abs() < 1e-9);
}

#[tokio::test]
async fn cross_80_threshold() {
    let (_w, pool, _tmp) = fixture().await;
    // Sonnet input is $3/Mtok → 2.7M input tokens = $8.10 → 81%.
    let (_, _, t) = budget::apply_report(
        &pool, "s1", "a1", None, "claude-3-5-sonnet", 2_700_000, 0,
    )
    .await
    .unwrap();
    assert_eq!(t, Some(budget::Threshold::Warn80));
}

#[tokio::test]
async fn cross_95_threshold() {
    let (_w, pool, _tmp) = fixture().await;
    // First push to 80% — trips Warn80.
    let (_, _, t1) = budget::apply_report(
        &pool, "s1", "a1", None, "claude-3-5-sonnet", 2_700_000, 0,
    )
    .await
    .unwrap();
    assert_eq!(t1, Some(budget::Threshold::Warn80));
    // Then to 96% (3.2M input → $9.60).
    let (_, _, t2) = budget::apply_report(
        &pool, "s1", "a1", None, "claude-3-5-sonnet", 500_000, 0,
    )
    .await
    .unwrap();
    assert_eq!(t2, Some(budget::Threshold::Warn95));
}

#[tokio::test]
async fn cross_100_triggers_pause_all() {
    let (w, pool, _tmp) = fixture().await;
    let mut out_bus: HashMap<String, mpsc::Sender<serde_json::Value>> = HashMap::new();
    let (tx, mut rx) = mpsc::channel::<serde_json::Value>(8);
    out_bus.insert("a1".to_string(), tx);

    // Push toward ~98%.
    budget::apply_report(&pool, "s1", "a1", None, "claude-3-5-sonnet", 3_300_000, 0)
        .await
        .unwrap();
    // Then push past 100%.
    let (spent, _, t) = budget::apply_report(
        &pool, "s1", "a1", None, "claude-3-5-sonnet", 200_000, 0,
    )
    .await
    .unwrap();
    assert_eq!(t, Some(budget::Threshold::Pause100));
    assert!(spent > 10.0);

    let sent = budget::pause_startup(&w, &out_bus, "s1");
    assert_eq!(sent, 1);

    let msg = rx.try_recv().expect("pause should be queued to a1's out_bus");
    assert_eq!(msg["type"], "pause");
    assert_eq!(msg["reason"], "budget_exhausted");
}

#[tokio::test]
async fn warn_startup_pushes_budget_warning() {
    // Spec §6.1: 80%/95% crossings must push a `budget_warning` frame to
    // every same-startup worker (not just write the system_events row).
    let (w, _pool, _tmp) = fixture().await;
    let mut out_bus: HashMap<String, mpsc::Sender<serde_json::Value>> = HashMap::new();
    let (tx, mut rx) = mpsc::channel::<serde_json::Value>(8);
    out_bus.insert("a1".to_string(), tx);

    let sent = budget::warn_startup(&w, &out_bus, "s1", 2.0, 80);
    assert_eq!(sent, 1);

    let msg = rx.try_recv().expect("budget_warning should be queued to a1's out_bus");
    assert_eq!(msg["type"], "budget_warning");
    assert_eq!(msg["v"], 1);
    assert_eq!(msg["remaining_usd"], 2.0);
    assert_eq!(msg["percent_used"], 80);
}

#[tokio::test]
async fn pause_startup_only_targets_matching_workers() {
    // a1 belongs to s1; a2 belongs to s2. Pausing s1 must NOT push to a2.
    let (mut w, pool, _tmp) = fixture().await;
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, budget_spent_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s2', 'beta', 'goal', 10.0, 0.0, 'town_default', '/tmp/s2', 'active', unixepoch())"
    ).execute(&pool).await.unwrap();
    w.avatars.insert(
        "a2".to_string(),
        AvatarView {
            agent_id: "a2".to_string(),
            startup_id: "s2".to_string(),
            role: "engineer".to_string(),
            backend: "claude_code".to_string(),
            current_pos: (0, 0),
            target_pos: None,
            room_id: "suite_2".to_string(),
            status: "idle".to_string(),
        },
    );

    let mut out_bus: HashMap<String, mpsc::Sender<serde_json::Value>> = HashMap::new();
    let (tx1, mut rx1) = mpsc::channel::<serde_json::Value>(8);
    let (tx2, mut rx2) = mpsc::channel::<serde_json::Value>(8);
    out_bus.insert("a1".to_string(), tx1);
    out_bus.insert("a2".to_string(), tx2);

    let sent = budget::pause_startup(&w, &out_bus, "s1");
    assert_eq!(sent, 1, "only a1 (in s1) should receive the pause");
    assert!(rx1.try_recv().is_ok());
    assert!(rx2.try_recv().is_err(), "a2 (in s2) must NOT receive a pause");
}

#[tokio::test]
async fn record_threshold_event_writes_system_event() {
    let (_w, pool, _tmp) = fixture().await;
    budget::record_threshold_event(&pool, "s1", budget::Threshold::Pause100, 10.5, 10.0)
        .await
        .unwrap();
    let count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM system_events WHERE kind='budget_pause_100' AND severity='alert'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count.0, 1);

    // Warn80 fires as severity=warn.
    budget::record_threshold_event(&pool, "s1", budget::Threshold::Warn80, 8.1, 10.0)
        .await
        .unwrap();
    let warn_count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM system_events WHERE kind='budget_warn_80' AND severity='warn'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(warn_count.0, 1);
}

#[tokio::test]
async fn raising_cap_prevents_re_trip() {
    // Push past 100%, then raise the cap. A subsequent small report should
    // NOT re-trip Pause100 because `newly_crossed` only fires on transitions.
    let (_w, pool, _tmp) = fixture().await;
    let (_, _, t) = budget::apply_report(&pool, "s1", "a1", None, "claude-3-5-sonnet", 4_000_000, 0)
        .await
        .unwrap();
    assert_eq!(t, Some(budget::Threshold::Pause100));

    // Operator raises the cap to $30 (current spend ~ $12).
    sqlx::query("UPDATE startups SET budget_cap_usd = 30.0 WHERE id='s1'")
        .execute(&pool)
        .await
        .unwrap();

    // Small additional report at the new cap shouldn't trip anything (we go
    // from ~$12 / $30 = 40% to ~$12.30 / $30 = 41%).
    let (_, _, t2) = budget::apply_report(&pool, "s1", "a1", None, "claude-3-5-sonnet", 100_000, 0)
        .await
        .unwrap();
    assert!(t2.is_none(), "raising the cap should auto-resume; got {:?}", t2);
}
