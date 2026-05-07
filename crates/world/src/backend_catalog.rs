use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
pub struct BackendInfo {
    pub id: String,
    pub available: bool,
    pub version: Option<String>,
    pub install_hint: Option<String>,
    pub last_checked_ts: i64,
}

pub async fn probe_all() -> HashMap<String, BackendInfo> {
    let mut out = HashMap::new();
    for (id, cmd, hint) in [
        ("claude_code", "claude", "Install: npm i -g @anthropic-ai/claude-code"),
        ("codex", "codex", "Install: npm i -g @openai/codex"),
        ("opencode", "opencode", "Install: see https://opencode.ai"),
    ] {
        out.insert(id.into(), probe_one(id, cmd, hint).await);
    }
    out
}

async fn probe_one(id: &str, cmd: &str, hint: &str) -> BackendInfo {
    let now = chrono::Utc::now().timestamp();
    let result = tokio::process::Command::new(cmd)
        .arg("--version")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(child) = result {
        if let Ok(Ok(out)) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            child.wait_with_output(),
        )
        .await
        {
            if out.status.success() {
                let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return BackendInfo {
                    id: id.into(),
                    available: true,
                    version: Some(version),
                    install_hint: None,
                    last_checked_ts: now,
                };
            }
        }
    }
    BackendInfo {
        id: id.into(),
        available: false,
        version: None,
        install_hint: Some(hint.into()),
        last_checked_ts: now,
    }
}
