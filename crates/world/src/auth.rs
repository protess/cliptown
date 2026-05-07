use anyhow::{anyhow, Result};
use sqlx::SqlitePool;

pub async fn validate_operator_token(_pool: &SqlitePool, token: &str) -> Result<()> {
    let expected = std::env::var("CLIPTOWN_OPERATOR_TOKEN").unwrap_or_else(|_| "dev-token".into());
    if token == expected { Ok(()) } else { Err(anyhow!("invalid_operator_token")) }
}

pub async fn validate_agent_secret(pool: &SqlitePool, agent_id: &str, secret: &str) -> Result<String> {
    // Plan v2 specified `SELECT startup_id, config_overrides FROM agents`, but the
    // 0001_initial.sql schema only defines `config_overrides` on `startups`, not on
    // `agents`. Selecting a nonexistent column would fail the query at runtime and
    // break the worker auth path. We only need `startup_id` here, so drop the unused
    // column. Documented as a justified deviation in the M1.3 report.
    let row: (String,) = sqlx::query_as("SELECT startup_id FROM agents WHERE id = ?")
        .bind(agent_id).fetch_optional(pool).await?
        .ok_or_else(|| anyhow!("unknown_agent"))?;
    let expected = std::env::var(format!("CLIPTOWN_AGENT_SECRET_{agent_id}")).unwrap_or_else(|_| "dev-secret".into());
    if secret != expected { return Err(anyhow!("invalid_agent_secret")); }
    Ok(row.0)
}
