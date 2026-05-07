//! World-side worker process supervisor. Spawns a Node worker per agent,
//! watches its lifecycle, and respawns with backoff on unexpected exit.
//!
//! Backoff schedule: [1s, 5s, 30s], 3 retries, then emit
//! `system_event { severity: alert, kind: "worker_dead" }`.
//!
//! On startup dissolve: SIGTERM all that startup's workers; 5s grace; SIGKILL.

use crate::persist;
use serde_json::json;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub const DEFAULT_BACKOFF_MS: [u64; 3] = [1_000, 5_000, 30_000];
pub const DISSOLVE_GRACE_MS: u64 = 5_000;

#[derive(Clone)]
pub struct SpawnConfig {
    pub agent_id: String,
    pub startup_id: String,
    pub world_url: String,
    pub secret: String,
    pub workspace: String,
    pub backend: String,
}

#[derive(Clone)]
pub struct SupervisorConfig {
    /// Path to the worker binary command. Phase 0 default: `node`.
    pub worker_bin: String,
    /// Args prepended before --agent-id, etc. Phase 0 default:
    /// ["packages/worker/dist/index.js"].
    pub worker_args: Vec<String>,
    pub backoff_ms: Vec<u64>,
    pub dissolve_grace_ms: u64,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        let bin = std::env::var("CLIPTOWN_WORKER_BIN").unwrap_or_else(|_| "node".to_string());
        let args = if bin == "node" {
            vec!["packages/worker/dist/index.js".to_string()]
        } else {
            vec![]
        };
        Self {
            worker_bin: bin,
            worker_args: args,
            backoff_ms: DEFAULT_BACKOFF_MS.to_vec(),
            dissolve_grace_ms: DISSOLVE_GRACE_MS,
        }
    }
}

/// Per-agent metadata held by the supervisor. The `Child` handle itself is
/// owned by the watch task (so it can call `wait()` exclusively); we only
/// keep the PID here so `dissolve_startup` can deliver signals concurrently.
struct AgentEntry {
    pid: Option<u32>,
    startup_id: String,
}

#[derive(Clone)]
pub struct AgentSupervisor {
    config: SupervisorConfig,
    pool: SqlitePool,
    /// Live agents the supervisor is watching. Insertion happens on spawn,
    /// removal happens when the watch loop exits (clean exit / max failures /
    /// dissolve). PID is updated on every respawn.
    agents: Arc<Mutex<HashMap<String, AgentEntry>>>,
    /// Agents intentionally terminated by `dissolve_startup`. The watch loop
    /// consults this on child exit to suppress respawn. Cleared when the
    /// watch loop has acted on it.
    tombstones: Arc<Mutex<HashSet<String>>>,
}

impl AgentSupervisor {
    pub fn new(config: SupervisorConfig, pool: SqlitePool) -> Self {
        Self {
            config,
            pool,
            agents: Arc::new(Mutex::new(HashMap::new())),
            tombstones: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Spawn a worker child for `cfg.agent_id`. Returns when the spawn syscall
    /// has succeeded; the watch task then takes ownership of the `Child` and
    /// runs the supervise/respawn loop until exhaustion or dissolve.
    pub async fn spawn_agent(self: &Arc<Self>, cfg: SpawnConfig) -> Result<(), std::io::Error> {
        let child = self.spawn_child(&cfg)?;
        let pid = child.id();
        {
            let mut map = self.agents.lock().await;
            map.insert(
                cfg.agent_id.clone(),
                AgentEntry {
                    pid,
                    startup_id: cfg.startup_id.clone(),
                },
            );
        }
        let sup = Arc::clone(self);
        let cfg2 = cfg.clone();
        tokio::spawn(async move {
            sup.watch_loop(cfg2, child).await;
        });
        Ok(())
    }

    fn spawn_child(&self, cfg: &SpawnConfig) -> Result<Child, std::io::Error> {
        let mut command = Command::new(&self.config.worker_bin);
        for a in &self.config.worker_args {
            command.arg(a);
        }
        command
            .arg("--world-url")
            .arg(&cfg.world_url)
            .arg("--agent-id")
            .arg(&cfg.agent_id)
            .arg("--startup-id")
            .arg(&cfg.startup_id)
            .arg("--secret")
            .arg(&cfg.secret)
            .arg("--workspace")
            .arg(&cfg.workspace)
            .arg("--backend")
            .arg(&cfg.backend);
        command.kill_on_drop(true);
        command.spawn()
    }

    async fn watch_loop(self: Arc<Self>, cfg: SpawnConfig, mut child: Child) {
        let mut failures = 0usize;
        loop {
            let exit_status = child.wait().await;

            // Was this exit caused by dissolve_startup? If so, suppress respawn.
            if self.take_tombstone(&cfg.agent_id).await {
                self.remove_agent(&cfg.agent_id).await;
                tracing::info!(component = "agent_supervisor", agent_id = %cfg.agent_id, "worker terminated by dissolve");
                return;
            }

            let succeeded = matches!(&exit_status, Ok(s) if s.success());
            if succeeded {
                self.remove_agent(&cfg.agent_id).await;
                tracing::info!(component = "agent_supervisor", agent_id = %cfg.agent_id, "worker exited cleanly");
                return;
            }

            failures += 1;
            tracing::warn!(component = "agent_supervisor",
                agent_id = %cfg.agent_id,
                failure = failures,
                "worker crashed; will retry after backoff"
            );

            if failures > self.config.backoff_ms.len() {
                let _ = persist::record_system_event(
                    &self.pool,
                    Some(&cfg.startup_id),
                    "worker_dead",
                    &json!({"agent_id": cfg.agent_id, "attempts": failures}).to_string(),
                    "alert",
                )
                .await;
                self.remove_agent(&cfg.agent_id).await;
                return;
            }

            let delay = self.config.backoff_ms[failures - 1];
            tokio::time::sleep(Duration::from_millis(delay)).await;

            // If dissolve raced during the backoff sleep, honor the tombstone.
            if self.take_tombstone(&cfg.agent_id).await {
                self.remove_agent(&cfg.agent_id).await;
                tracing::info!(component = "agent_supervisor", agent_id = %cfg.agent_id, "respawn cancelled by dissolve");
                return;
            }

            let new_child = match self.spawn_child(&cfg) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(component = "agent_supervisor", agent_id = %cfg.agent_id, error = %e, "respawn failed");
                    continue;
                }
            };
            // Update the recorded PID so dissolve targets the new process.
            {
                let mut map = self.agents.lock().await;
                if let Some(entry) = map.get_mut(&cfg.agent_id) {
                    entry.pid = new_child.id();
                }
            }
            child = new_child;
        }
    }

    async fn remove_agent(&self, agent_id: &str) {
        let mut map = self.agents.lock().await;
        map.remove(agent_id);
    }

    async fn take_tombstone(&self, agent_id: &str) -> bool {
        let mut set = self.tombstones.lock().await;
        set.remove(agent_id)
    }

    async fn set_tombstone(&self, agent_id: &str) {
        let mut set = self.tombstones.lock().await;
        set.insert(agent_id.to_string());
    }

    /// SIGTERM all workers of `startup_id`, wait grace, SIGKILL stragglers.
    pub async fn dissolve_startup(&self, startup_id: &str) {
        // Snapshot (agent_id, pid) for this startup. We tombstone every match
        // so the watch loop suppresses respawn even if the kill races.
        let targets: Vec<(String, Option<u32>)> = {
            let map = self.agents.lock().await;
            map.iter()
                .filter(|(_, e)| e.startup_id == startup_id)
                .map(|(k, e)| (k.clone(), e.pid))
                .collect()
        };

        for (agent_id, _) in &targets {
            self.set_tombstone(agent_id).await;
        }

        // Phase 1: SIGTERM
        for (_, pid) in &targets {
            if let Some(pid) = pid {
                Self::send_signal(*pid as i32, libc::SIGTERM);
            }
        }

        // Phase 2: grace
        tokio::time::sleep(Duration::from_millis(self.config.dissolve_grace_ms)).await;

        // Phase 3: SIGKILL stragglers. We re-read PIDs in case a respawn
        // happened mid-grace (extremely unlikely given tombstone, but safe).
        let stragglers: Vec<Option<u32>> = {
            let map = self.agents.lock().await;
            targets
                .iter()
                .map(|(id, _)| map.get(id).and_then(|e| e.pid))
                .collect()
        };
        for pid in stragglers.into_iter().flatten() {
            Self::send_signal(pid as i32, libc::SIGKILL);
        }
        // The watch loop removes each agent from `agents` after observing
        // the child's exit; we don't remove here to avoid double-remove races.
    }

    fn send_signal(pid: i32, sig: libc::c_int) {
        // SAFETY: libc::kill is safe to call with a process id we own.
        unsafe {
            libc::kill(pid, sig);
        }
    }

    pub async fn agent_count(&self) -> usize {
        self.agents.lock().await.len()
    }
}
