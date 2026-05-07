use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub world: WorldCfg,
    pub task: TaskCfg,
    pub epistemic: EpistemicCfg,
    pub budget: BudgetCfg,
    pub supervisor: SupervisorCfg,
    pub possess: PossessCfg,
    pub kanban: KanbanCfg,
}

#[derive(Debug, Deserialize)]
pub struct WorldCfg { pub tick_hz: u32, pub position_snapshot_every_ticks: u32 }

#[derive(Debug, Deserialize)]
pub struct TaskCfg { pub max_review_rounds: u32, pub max_llm_turns_per_task: u32 }

#[derive(Debug, Deserialize)]
pub struct EpistemicCfg {
    pub max_hypotheses_per_task: u32,
    pub max_tests_per_hypothesis: u32,
    pub non_trivial_description_token_threshold: u32,
}

#[derive(Debug, Deserialize)]
pub struct BudgetCfg { pub warn_pct: u32, pub no_new_task_pct: u32, pub pause_all_pct: u32 }

#[derive(Debug, Deserialize)]
pub struct SupervisorCfg {
    pub worker_respawn_backoff_seconds: Vec<u64>,
    pub worker_respawn_max_attempts: u32,
}

#[derive(Debug, Deserialize)]
pub struct PossessCfg { pub operator_keepalive_timeout_seconds: u64 }

#[derive(Debug, Deserialize)]
pub struct KanbanCfg { pub stuck_warn_minutes: u32, pub stuck_alert_minutes: u32 }

pub fn load_from(path: &str) -> Result<Config> {
    let s = std::fs::read_to_string(path)?;
    let cfg: Config = toml::from_str(&s)?;
    Ok(cfg)
}
