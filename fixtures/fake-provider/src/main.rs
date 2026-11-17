//! `fake-provider`: the fault-injecting fake provider over HTTP (a test fixture).
//!
//! Environment: `BP_FAKE_LISTEN` (default `127.0.0.1:61410`), `BP_FAKE_UNIQUE_NAMES`
//! (`true` to refuse taken names), `BP_FAKE_IDEMPOTENCY_KEYS` (`true` to de-duplicate
//! creates by key), `BP_FAKE_RECORD_CALLS` (`true` to keep every call),
//! `BP_FAKE_HANG_MS` (default 30000), `BP_FAKE_RANDOM_PER_MILLION` and `BP_FAKE_RANDOM_SEED`
//! (a random fault schedule from the start).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use platform_fake_provider::server::{AppState, router};
use platform_fake_provider::{DEFAULT_HANG, FakeOptions, FakeProvider, FaultConfig, RandomFaults};

fn env_bool(name: &str) -> anyhow::Result<bool> {
    match std::env::var(name).as_deref() {
        Err(_) | Ok("" | "false" | "0") => Ok(false),
        Ok("true" | "1") => Ok(true),
        Ok(other) => anyhow::bail!("{name} must be true or false, not {other:?}"),
    }
}

fn env_parse<T: std::str::FromStr>(name: &str) -> anyhow::Result<Option<T>>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Ok(Some(
            value.parse().with_context(|| format!("parsing {name}"))?,
        )),
        _ => Ok(None),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let listen: SocketAddr =
        env_parse("BP_FAKE_LISTEN")?.unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 61410)));
    let hang = env_parse::<u64>("BP_FAKE_HANG_MS")?.map_or(DEFAULT_HANG, Duration::from_millis);
    let random = env_parse::<u32>("BP_FAKE_RANDOM_PER_MILLION")?.map(|per_million| {
        Ok::<_, anyhow::Error>(RandomFaults {
            seed: env_parse("BP_FAKE_RANDOM_SEED")?.unwrap_or(1),
            per_million,
            max: None,
        })
    });
    let random = random.transpose()?;
    let fake = Arc::new(FakeProvider::new(FakeOptions {
        unique_names: env_bool("BP_FAKE_UNIQUE_NAMES")?,
        honor_idempotency_keys: env_bool("BP_FAKE_IDEMPOTENCY_KEYS")?,
        record_calls: env_bool("BP_FAKE_RECORD_CALLS")?,
        faults: FaultConfig {
            rules: Vec::new(),
            random,
        },
        latency: Duration::ZERO,
    }));
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    eprintln!("fake-provider listening on {}", listener.local_addr()?);
    axum::serve(listener, router(AppState { fake, hang }))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("serving")?;
    Ok(())
}
