use cliptown_world::agent_supervisor::{
    per_task_workers_enabled, AgentSupervisor, SpawnConfig, SupervisorConfig, TaskSpawn,
};
use cliptown_world::storage;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Serialize tests that mutate process env so they don't race under `cargo
/// test`'s default thread pool. Three tests in this file touch
/// `CLIPTOWN_TEST_ARGS_FILE` / `CLIPTOWN_PER_TASK_WORKERS`; without this
/// guard they corrupt each other's expectations when run in parallel.
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|p| p.into_inner())
}

fn make_event_tx() -> tokio::sync::broadcast::Sender<cliptown_world::protocol::ConsoleOutbound> {
    tokio::sync::broadcast::channel(64).0
}

fn make_event_channel() -> (
    tokio::sync::broadcast::Sender<cliptown_world::protocol::ConsoleOutbound>,
    tokio::sync::broadcast::Receiver<cliptown_world::protocol::ConsoleOutbound>,
) {
    tokio::sync::broadcast::channel(64)
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
        task: None,
    }
}

// ── P3 Theme C follow-up (Option B): per-task spawn ────────────────────────

/// `per_task_workers_enabled()` toggles purely off the env var. The test
/// touches the global env so it runs serially via tokio's default single-
/// threaded runtime, and clears the var afterwards.
#[tokio::test]
async fn per_task_workers_enabled_reads_env_var() {
    let _g = env_lock();
    std::env::remove_var("CLIPTOWN_PER_TASK_WORKERS");
    assert!(!per_task_workers_enabled());
    std::env::set_var("CLIPTOWN_PER_TASK_WORKERS", "1");
    assert!(per_task_workers_enabled());
    std::env::set_var("CLIPTOWN_PER_TASK_WORKERS", "0");
    assert!(!per_task_workers_enabled(), "only literal '1' enables");
    std::env::remove_var("CLIPTOWN_PER_TASK_WORKERS");
}

/// When `cfg.task` is `Some`, the supervisor adds `--real --task-id --prompt`
/// plus any preferred-override flags to the worker command. Verified by a
/// fixture script that dumps its argv to a file.
#[tokio::test]
async fn spawn_with_task_passes_real_and_preferred_flags() {
    let _g = env_lock();
    let (pool, dir) = fixture().await;
    let args_file = dir.path().join("argv.txt");
    std::env::set_var("CLIPTOWN_TEST_ARGS_FILE", &args_file);
    let sup = Arc::new(AgentSupervisor::new(
        config_for("fake_worker_dump_args.sh"),
        pool,
        make_event_tx(),
    ));
    let mut cfg = spawn_cfg("a1", "s1");
    cfg.task = Some(TaskSpawn {
        task_id: "T7".to_string(),
        prompt: "do the thing".to_string(),
        preferred_backend: Some("codex".to_string()),
        preferred_model: Some("gpt-5-mini".to_string()),
    });
    sup.spawn_agent(cfg).await.unwrap();
    // Wait for the clean exit + dump.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let content = std::fs::read_to_string(&args_file).expect("argv dump file");
    let lines: Vec<&str> = content.lines().collect();
    assert!(lines.contains(&"--real"), "argv missing --real: {lines:?}");
    let pair = |flag: &str, val: &str| {
        let idx = lines.iter().position(|l| *l == flag).expect(&format!("missing {flag}"));
        assert_eq!(lines.get(idx + 1).copied(), Some(val), "{flag} val mismatch");
    };
    pair("--task-id", "T7");
    pair("--prompt", "do the thing");
    pair("--preferred-backend", "codex");
    pair("--preferred-model", "gpt-5-mini");
    pair("--backend", "claude_code"); // default still wired
    std::env::remove_var("CLIPTOWN_TEST_ARGS_FILE");
}

/// When `cfg.task` is `None` (legacy daemon path), `--real` and the per-task
/// flags are absent. Same fixture, different cfg shape.
#[tokio::test]
async fn spawn_without_task_omits_real_and_preferred_flags() {
    let _g = env_lock();
    let (pool, dir) = fixture().await;
    let args_file = dir.path().join("argv.txt");
    std::env::set_var("CLIPTOWN_TEST_ARGS_FILE", &args_file);
    let sup = Arc::new(AgentSupervisor::new(
        config_for("fake_worker_dump_args.sh"),
        pool,
        make_event_tx(),
    ));
    sup.spawn_agent(spawn_cfg("a1", "s1")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let content = std::fs::read_to_string(&args_file).expect("argv dump file");
    let lines: Vec<&str> = content.lines().collect();
    assert!(!lines.contains(&"--real"), "argv unexpectedly has --real: {lines:?}");
    assert!(!lines.contains(&"--task-id"), "argv unexpectedly has --task-id");
    assert!(!lines.contains(&"--preferred-backend"));
    assert!(!lines.contains(&"--preferred-model"));
    std::env::remove_var("CLIPTOWN_TEST_ARGS_FILE");
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
    let (event_tx, mut event_rx) = make_event_channel();
    let sup = Arc::new(AgentSupervisor::new(
        config_for("fake_worker_crash.sh"),
        pool.clone(),
        event_tx,
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

    // Verify the worker_dead SystemEvent broadcast reaches operator consoles.
    let frame = event_rx.try_recv().expect("expected SystemEvent broadcast for worker_dead");
    let cliptown_world::protocol::ConsoleOutbound::SystemEvent { kind, severity, .. } = frame else {
        panic!("expected SystemEvent, got {:?}", frame);
    };
    assert_eq!(kind, "worker_dead");
    assert_eq!(severity, "alert");
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
