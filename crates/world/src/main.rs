use anyhow::Result;
use cliptown_world::{config, http, loop_, state::WorldView, storage};
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    tracing::info!(component = "world", event = "boot");

    let _cfg = config::load_from("cliptown.toml")?;
    let db_path = std::env::var("CLIPTOWN_DB").unwrap_or_else(|_| "cliptown.db".into());
    let pool = storage::open(&db_path).await?;
    tracing::info!(component = "world", event = "storage_ready", db = %db_path);

    let handle = loop_::spawn(WorldView::default());
    let catalog = std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
    tracing::info!(component = "world", event = "loop_started");

    let app = http::router(http::AppState { pool, handle, catalog });
    let addr = std::env::var("CLIPTOWN_ADDR").unwrap_or_else(|_| "127.0.0.1:0".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let bound = listener.local_addr()?;
    tracing::info!(component = "world", event = "listening", addr = %bound);
    axum::serve(listener, app).await?;
    Ok(())
}
