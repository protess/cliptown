use cliptown_world::agent_supervisor::{AgentSupervisor, SpawnConfig, SupervisorConfig};
use cliptown_world::storage;
use std::sync::Arc;
use std::time::Duration;

fn make_event_tx() -> tokio::sync::broadcast::Sender<cliptown_world::protocol::ConsoleOutbound> {
    tokio::sync::broadcast::channel(64).0
}

async fn fixture() -> (sqlx::SqlitePool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("test.db");
    let pool = storage::open(p.to_str().unwrap()).await.unwrap();
    sqlx::query("INSERT INTO towns (id, name, map_json) VALUES ('town_default', 'T', '{}')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s1', 'alpha', 'goal', 10.0, 'town_default', '/tmp/s1', 'active', unixepoch())"
    )
    .execute(&pool)
    .await
    .unwrap();
    (pool, dir)
}

fn fixture_path(name: &str) -> String {
    let cargo_dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/tests/fixtures/{}", cargo_dir, name)
}

fn config_for(script: &str) -> SupervisorConfig {
    SupervisorConfig {
        worker_bin: "/bin/sh".to_string(),
        worker_args: vec![fixture_path(script)],
        // Small backoffs keep tests fast; the default is [1s, 5s, 30s].
        backoff_ms: vec![10, 20, 30],
        dissolve_grace_ms: 100,
    }
}

fn spawn_cfg(agent_id: &str, startup_id: &str) -> SpawnConfig {
    SpawnConfig {
        agent_id: agent_id.to_string(),
        startup_id: startup_id.to_string(),
        world_url: "ws://127.0.0.1:9999/ws/worker".to_string(),
        secret: "test-secret".to_string(),
        workspace: "/tmp/test-ws".to_string(),
        backend: "claude_code".to_string(),
    }
}

#[tokio::test]
async fn clean_exit_does_not_respawn() {
    let (pool, _dir) = fixture().await;
    let sup = Arc::new(AgentSupervisor::new(
        config_for("fake_worker_clean_exit.sh"),
        pool,
        make_event_tx(),
    ));
    sup.spawn_agent(spawn_cfg("a1", "s1")).await.unwrap();
    // Wait for the watch loop to observe the clean exit and remove the agent.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(sup.agent_count().await, 0);
}

#[tokio::test]
async fn crash_respawns_with_backoff_then_alerts() {
    let (pool, _dir) = fixture().await;
    let sup = Arc::new(AgentSupervisor::new(
        config_for("fake_worker_crash.sh"),
        pool.clone(),
        make_event_tx(),
    ));
    sup.spawn_agent(spawn_cfg("a2", "s1")).await.unwrap();
    // 1 initial + 3 retries with backoff [10,20,30]ms ≈ 60ms backoff +
    // ~4 × ~20ms exec; allow a generous ceiling for slow CI.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM system_events WHERE kind = 'worker_dead' AND severity = 'alert'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count.0, 1, "supervisor should emit one worker_dead alert");
    assert_eq!(sup.agent_count().await, 0, "agent should be removed after exhaustion");
}

#[tokio::test]
async fn dissolve_kills_only_targeted_startups_workers() {
    let (pool, _dir) = fixture().await;
    sqlx::query(
        "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
         VALUES ('s2', 'beta', 'goal', 10.0, 'town_default', '/tmp/s2', 'active', unixepoch())"
    )
    .execute(&pool)
    .await
    .unwrap();

    let sup = Arc::new(AgentSupervisor::new(
        config_for("fake_worker_long_run.sh"),
        pool,
        make_event_tx(),
    ));
    sup.spawn_agent(spawn_cfg("a1", "s1")).await.unwrap();
    sup.spawn_agent(spawn_cfg("b1", "s2")).await.unwrap();
    // Give both children a moment to actually be running.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(sup.agent_count().await, 2);

    sup.dissolve_startup("s1").await;
    // Watch loop needs a tick to observe the kill, take the tombstone,
    // and remove the agent.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        sup.agent_count().await,
        1,
        "s1's worker should be dead; s2's should remain"
    );
}
