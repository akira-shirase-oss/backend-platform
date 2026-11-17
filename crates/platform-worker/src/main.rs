//! `platform-worker`: the reconciler process. It opens no listening socket (T2).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use platform_adapters::{HttpProvider, HttpProviderConfig};
use platform_worker::PgLog;
use platform_worker::config::{Config, LogFormat};
use platform_worker::worker::{WorkerConfig, run};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

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
    let settings = platform_db::PoolSettings {
        max_connections: config.db_max_connections,
        ..platform_db::PoolSettings::default()
    };
    let pool = platform_db::connect_lazy(config.database_url.expose(), &settings)
        .context("configuring the pool")?;
    // Refuse to run as a role that could rewrite the operation log (F1).
    platform_db::guard::ensure_at_startup(&pool, Duration::from_secs(30))
        .await
        .map_err(|e| anyhow::anyhow!(platform_db::chain(&e)))
        .context("checking the runtime role")?;
    let provider = HttpProvider::new(&HttpProviderConfig {
        base_url: config.provider_url.clone(),
        connect_timeout: Duration::from_millis(500),
        request_timeout: config.reconcile.call_timeout,
    })
    .context("building the provider client")?;
    let worker = WorkerConfig {
        id: Uuid::now_v7(),
        concurrency: config.concurrency,
        poll_idle_max: config.poll_idle_max,
        reconcile: config.reconcile,
    };
    tracing::info!(worker = %worker.id, concurrency = worker.concurrency, "worker starting");
    let cancel = CancellationToken::new();
    let log = Arc::new(PgLog::new(pool.clone()));
    let provider = Arc::new(provider);
    let running = tokio::spawn(start(log, provider, worker, cancel.clone()));

    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("installing the SIGTERM handler")?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
    tracing::info!("shutdown signal; finishing rounds in progress");
    cancel.cancel();
    if tokio::time::timeout(config.shutdown_grace, running)
        .await
        .is_err()
    {
        tracing::warn!(
            "shutdown deadline exceeded; leases will expire and the operations resume elsewhere"
        );
    }
    pool.close().await;
    Ok(())
}

#[cfg(not(feature = "fault-injection"))]
async fn start(
    log: Arc<PgLog>,
    provider: Arc<HttpProvider>,
    worker: WorkerConfig,
    cancel: CancellationToken,
) {
    run(
        log,
        provider,
        Arc::new(platform_worker::NoHooks),
        worker,
        cancel,
    )
    .await;
}

#[cfg(feature = "fault-injection")]
async fn start(
    log: Arc<PgLog>,
    provider: Arc<HttpProvider>,
    worker: WorkerConfig,
    cancel: CancellationToken,
) {
    match halt::HaltAt::from_env() {
        Ok(Some(hooks)) => run(log, provider, Arc::new(hooks), worker, cancel).await,
        Ok(None) => {
            run(
                log,
                provider,
                Arc::new(platform_worker::NoHooks),
                worker,
                cancel,
            )
            .await;
        }
        Err(problem) => tracing::error!(%problem, "BP_HALT_AT is invalid; not starting"),
    }
}

/// Fault-injection builds only: `BP_HALT_AT=<point>:<position>` parks the worker forever at
/// that point (holding its lease and connections) so the drill can `kill -9` it there.
#[cfg(feature = "fault-injection")]
mod halt {
    use platform_worker::{Halted, Hooks, Point};

    pub(crate) struct HaltAt {
        point: Point,
        position: u16,
    }

    impl HaltAt {
        pub(crate) fn from_env() -> Result<Option<Self>, String> {
            let Ok(spec) = std::env::var("BP_HALT_AT") else {
                return Ok(None);
            };
            let (point, position) = spec.split_once(':').ok_or("expected <point>:<position>")?;
            Ok(Some(Self {
                point: Point::parse(point).ok_or("unknown point")?,
                position: position.parse().map_err(|_| "position must be a number")?,
            }))
        }
    }

    impl Hooks for HaltAt {
        async fn at(&self, point: Point, position: u16, attempt: u16) -> Result<(), Halted> {
            if point == self.point && (position == self.position || point == Point::AfterFinish) {
                eprintln!(
                    "bp-worker halted at {} step {position} attempt {attempt}",
                    point.as_str()
                );
                std::future::pending::<()>().await;
            }
            Ok(())
        }
    }
}
