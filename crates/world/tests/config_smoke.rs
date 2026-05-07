#[test]
fn loads_repo_root_config() {
    // Tests run from crates/world/, so ../../cliptown.toml is the repo-root file.
    let cfg = cliptown_world::config::load_from("../../cliptown.toml").unwrap();
    assert_eq!(cfg.world.tick_hz, 1);
    assert_eq!(cfg.task.max_review_rounds, 3);
    assert_eq!(cfg.kanban.stuck_alert_minutes, 30);
    assert_eq!(cfg.supervisor.worker_respawn_backoff_seconds, vec![1, 5, 30]);
    assert_eq!(cfg.budget.pause_all_pct, 100);
}
