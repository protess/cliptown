use anyhow::{anyhow, Result};
use sqlx::SqlitePool;

/// P3 Theme B: operator roles, ascending privilege.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorRole {
    Viewer = 0,
    Manager = 1,
    Admin = 2,
}

impl OperatorRole {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "viewer" => Some(Self::Viewer),
            "manager" => Some(Self::Manager),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Manager => "manager",
            Self::Admin => "admin",
        }
    }

    /// Privilege ladder check. `a.at_least(b)` returns true iff `a` has
    /// at least the privilege of `b`.
    pub fn at_least(self, other: OperatorRole) -> bool {
        (self as u8) >= (other as u8)
    }
}

/// Identified operator after a successful token validation.
#[derive(Debug, Clone)]
pub struct OperatorIdentity {
    pub id: String,
    pub name: String,
    pub role: OperatorRole,
}

impl OperatorIdentity {
    /// Synthetic admin identity for integration tests that invoke
    /// `cmd_console::dispatch` directly without going through the WS
    /// authentication path. Lives outside #[cfg(test)] because it's used by
    /// crate-level integration tests in `tests/`, which compile as separate
    /// crates and can't see #[cfg(test)] items.
    pub fn admin_for_tests() -> Self {
        Self {
            id: "op_test".to_string(),
            name: "test-admin".to_string(),
            role: OperatorRole::Admin,
        }
    }
}

/// P3 Theme B: validate a console bearer token against the `operators`
/// table; fall back to `CLIPTOWN_OPERATOR_TOKEN` env var for backward
/// compatibility with deployments that haven't migrated to the table-
/// only model. The env-var path returns a synthetic admin identity.
pub async fn validate_operator_token(
    pool: &SqlitePool,
    token: &str,
) -> Result<OperatorIdentity> {
    if token.is_empty() {
        return Err(anyhow!("invalid_operator_token"));
    }
    // 1. Table lookup.
    let row: Option<(String, String, String)> =
        sqlx::query_as("SELECT id, name, role FROM operators WHERE token = ?")
            .bind(token)
            .fetch_optional(pool)
            .await?;
    if let Some((id, name, role_str)) = row {
        let role = OperatorRole::from_str(&role_str)
            .ok_or_else(|| anyhow!("unknown_operator_role: {role_str}"))?;
        return Ok(OperatorIdentity { id, name, role });
    }
    // 2. Env-var fallback (legacy path; retired once operators table is
    //    the sole source of truth).
    let env_token = std::env::var("CLIPTOWN_OPERATOR_TOKEN")
        .unwrap_or_else(|_| "dev-token".into());
    if token == env_token {
        return Ok(OperatorIdentity {
            id: "op_env".to_string(),
            name: "env-admin".to_string(),
            role: OperatorRole::Admin,
        });
    }
    Err(anyhow!("invalid_operator_token"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage;

    async fn fresh_pool() -> SqlitePool {
        let dir = tempfile::tempdir().unwrap();
        let pool = storage::open(dir.path().join("t.db").to_str().unwrap())
            .await
            .unwrap();
        crate::seed::seed_if_empty(&pool).await.unwrap();
        std::mem::forget(dir);
        pool
    }

    #[tokio::test]
    async fn validate_operator_token_via_seeded_admin_returns_admin_role() {
        let pool = fresh_pool().await;
        // Migration 0003 seeds op_default with token=dev-token role=admin.
        let id = validate_operator_token(&pool, "dev-token").await.unwrap();
        assert_eq!(id.role, OperatorRole::Admin);
        assert_eq!(id.id, "op_default");
        assert_eq!(id.name, "default-admin");
    }

    #[tokio::test]
    async fn validate_operator_token_rejects_unknown_and_empty() {
        let pool = fresh_pool().await;
        assert!(validate_operator_token(&pool, "wrong").await.is_err());
        assert!(validate_operator_token(&pool, "").await.is_err());
    }

    #[tokio::test]
    async fn validate_operator_token_supports_viewer_and_manager_rows() {
        let pool = fresh_pool().await;
        sqlx::query("INSERT INTO operators (id, name, token, role, created_at) VALUES ('op_v','viewer-bot','tok_v','viewer',unixepoch())")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO operators (id, name, token, role, created_at) VALUES ('op_m','manager-bot','tok_m','manager',unixepoch())")
            .execute(&pool).await.unwrap();
        let v = validate_operator_token(&pool, "tok_v").await.unwrap();
        let m = validate_operator_token(&pool, "tok_m").await.unwrap();
        assert_eq!(v.role, OperatorRole::Viewer);
        assert_eq!(m.role, OperatorRole::Manager);
    }

    #[test]
    fn role_at_least_orders_correctly() {
        use OperatorRole::*;
        assert!(Admin.at_least(Viewer));
        assert!(Admin.at_least(Manager));
        assert!(Admin.at_least(Admin));
        assert!(Manager.at_least(Viewer));
        assert!(Manager.at_least(Manager));
        assert!(!Manager.at_least(Admin));
        assert!(Viewer.at_least(Viewer));
        assert!(!Viewer.at_least(Manager));
        assert!(!Viewer.at_least(Admin));
    }
}
