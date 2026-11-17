//! `platform-server`: load config, connect, migrate, serve, drain.

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
    if config.migrate_on_start {
        migrate(&db).await?;
    }
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

/// Applies migrations, retrying while the database starts, for at most 30 s.
async fn migrate(db: &sqlx::PgPool) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut delay = Duration::from_millis(200);
    loop {
        match tokio::time::timeout(Duration::from_secs(20), platform_db::MIGRATOR.run(db)).await {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) if tokio::time::Instant::now() + delay < deadline => {
                tracing::warn!(%error, "migrations failed; retrying");
            }
            Ok(Err(error)) => return Err(error).context("applying migrations"),
            Err(_) => anyhow::bail!("applying migrations exceeded 20 s"),
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(2));
    }
}
