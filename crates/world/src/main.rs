use anyhow::Result;
use cliptown_world::{backend_catalog, config, http, loop_, seed, state::WorldView, storage};
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

    seed::seed_if_empty(&pool).await?;
    tracing::info!(component = "world", event = "town_seeded", town_id = "town_default");

    let handle = loop_::spawn(WorldView::default(), pool.clone());
    tracing::info!(component = "world", event = "loop_started");

    // Boot probe — populate catalog before serving traffic
    let initial_cat = backend_catalog::probe_all().await;
    let initial_json: std::collections::HashMap<String, serde_json::Value> = initial_cat
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap()))
        .collect();
    let catalog = std::sync::Arc::new(tokio::sync::RwLock::new(initial_json.clone()));
    let _ = handle
        .tx
        .send(loop_::Cmd::BackendCatalogUpdated(initial_json))
        .await;
    tracing::info!(
        component = "world",
        event = "backend_catalog_probed",
        count = initial_cat.len()
    );

    // 5-min refresh
    {
        let cat = catalog.clone();
        let h = handle.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
            tick.tick().await; // consume the immediate-fire
            loop {
                tick.tick().await;
                let new_cat = backend_catalog::probe_all().await;
                let new_json: std::collections::HashMap<_, _> = new_cat
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap()))
                    .collect();
                *cat.write().await = new_json.clone();
                let _ = h.tx.send(loop_::Cmd::BackendCatalogUpdated(new_json)).await;
                tracing::info!(component = "world", event = "backend_catalog_refreshed");
            }
        });
    }

    // SIGHUP → recheck (Unix only)
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let cat = catalog.clone();
        let h = handle.clone();
        tokio::spawn(async move {
            let mut s = signal(SignalKind::hangup()).unwrap();
            while s.recv().await.is_some() {
                let new_cat = backend_catalog::probe_all().await;
                let new_json: std::collections::HashMap<_, _> = new_cat
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap()))
                    .collect();
                *cat.write().await = new_json.clone();
                let _ = h.tx.send(loop_::Cmd::BackendCatalogUpdated(new_json)).await;
                tracing::info!(component = "world", event = "backend_catalog_recheck_sighup");
            }
        });
    }

    let app = http::router(http::AppState { pool, handle, catalog });
    let addr = std::env::var("CLIPTOWN_ADDR").unwrap_or_else(|_| "127.0.0.1:0".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let bound = listener.local_addr()?;
    tracing::info!(component = "world", event = "listening", addr = %bound);
    axum::serve(listener, app).await?;
    Ok(())
}
