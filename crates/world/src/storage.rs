use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

pub async fn open(path: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(path)?
        .create_if_missing(true)
        .pragma("journal_mode", "WAL")
        .pragma("synchronous", "NORMAL")
        .pragma("foreign_keys", "ON")
        .busy_timeout(std::time::Duration::from_millis(5000));
    let pool = SqlitePoolOptions::new().connect_with(opts).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
