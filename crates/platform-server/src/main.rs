//! `platform-server`: load config, connect, check the runtime role, serve, drain.

use std::time::Duration;

use anyhow::Context as _;
use platform_server::config::LogFormat;
use platform_server::{AppState, Config, app, shutdown};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()
        .map_err(anyhow::Error::msg)
        .context("loading configuration")?;
    let filter =
        tracing_subscriber::EnvFilter::try_new(&config.log_filter).context("parsing BP_LOG")?;
    match config.log_format {
        LogFormat::Text => tracing_subscriber::fmt().with_env_filter(filter).init(),
        LogFormat::Json => tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .init(),
    }
    let signal = shutdown::os_signal().context("installing signal handlers")?;
    let settings = platform_db::PoolSettings {
        max_connections: config.db_max_connections,
        ..platform_db::PoolSettings::default()
    };
    let db = platform_db::connect_lazy(config.database_url.expose(), &settings)
        .context("configuring the pool")?;
    // The server never migrates (`platform-dbctl migrate` does, as the owner) and refuses
    // to run as a role that could rewrite the operation log (F1).
    platform_db::guard::ensure_at_startup(&db, Duration::from_secs(30))
        .await
        .map_err(|e| anyhow::anyhow!(platform_db::chain(&e)))
        .context("checking the runtime role")?;
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("binding {}", config.listen))?;
    let local = listener.local_addr().context("reading the bound address")?;
    let router = app(&config, AppState::new(&config, db.clone()));
    tracing::info!(%local, "listening");
    let outcome = shutdown::serve(listener, router, config.shutdown_grace, async move {
        let name = signal.await;
        tracing::info!(signal = name, "shutdown signal received; draining");
    })
    .await
    .context("serving")?;
    if outcome == shutdown::ShutdownOutcome::DeadlineExceeded {
        tracing::warn!("shutdown deadline exceeded; exiting with requests in flight");
    }
    db.close().await;
    Ok(())
}
