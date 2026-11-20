//! Configuration from the environment: typed, validated once, every problem reported.

use std::time::Duration;

use platform_domain::RetryPolicy;

use crate::reconcile::ReconcileConfig;

/// A secret that never prints.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// The value, for the one place that needs it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable.
    Text,
    /// One JSON object per line.
    Json,
}

/// The worker's configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// The control database (it holds credentials, so it is a secret).
    pub database_url: Secret,
    /// The provider endpoint.
    pub provider_url: String,
    /// Concurrent reconcile tasks.
    pub concurrency: usize,
    /// Pool size.
    pub db_max_connections: u32,
    /// Longest idle poll interval.
    pub poll_idle_max: Duration,
    /// Client-side deadline on each operation-log call (F12).
    pub db_call_timeout: Duration,
    /// Deadlines and budgets.
    pub reconcile: ReconcileConfig,
    /// How long shutdown waits for in-progress rounds.
    pub shutdown_grace: Duration,
    /// Log filter.
    pub log_filter: String,
    /// Log format.
    pub log_format: LogFormat,
}

fn number<T: std::str::FromStr>(
    var: &dyn Fn(&str) -> Option<String>,
    name: &str,
    default: T,
    errors: &mut Vec<String>,
) -> T {
    match var(name) {
        None => default,
        Some(value) => value.parse().unwrap_or_else(|_| {
            errors.push(format!("{name} must be a number, not {value:?}"));
            default
        }),
    }
}

impl Config {
    /// Reads and validates the environment, reporting every problem at once.
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// The same validation over an injected lookup, so tests and fuzz targets can feed
    /// arbitrary values without mutating the process environment.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let var = |name: &str| lookup(name).filter(|v| !v.is_empty());
        let mut errors = Vec::new();
        let database_url = var("BP_DATABASE_URL").unwrap_or_else(|| {
            errors.push("BP_DATABASE_URL is required".to_owned());
            String::new()
        });
        let provider_url = var("BP_PROVIDER_URL").unwrap_or_else(|| {
            errors.push("BP_PROVIDER_URL is required".to_owned());
            String::new()
        });
        if !provider_url.is_empty()
            && !provider_url.starts_with("http://")
            && !provider_url.starts_with("https://")
        {
            errors.push("BP_PROVIDER_URL must be an http(s) URL".to_owned());
        }
        let concurrency = number(&var, "BP_WORKER_CONCURRENCY", 16_usize, &mut errors);
        if !(1..=256).contains(&concurrency) {
            errors.push("BP_WORKER_CONCURRENCY must be 1 to 256".to_owned());
        }
        let default_pool = u32::try_from(concurrency + 2).unwrap_or(32);
        let db_max_connections = number(&var, "BP_DB_MAX_CONNECTIONS", default_pool, &mut errors);
        let reconcile = ReconcileConfig {
            policy: RetryPolicy {
                max_attempts: number(&var, "BP_MAX_ATTEMPTS", 5, &mut errors),
                base: Duration::from_millis(number(&var, "BP_BACKOFF_BASE_MS", 50, &mut errors)),
                cap: Duration::from_millis(number(&var, "BP_BACKOFF_CAP_MS", 2_000, &mut errors)),
            },
            call_timeout: Duration::from_millis(number(
                &var,
                "BP_CALL_TIMEOUT_MS",
                2_000,
                &mut errors,
            )),
            lease: Duration::from_millis(number(&var, "BP_LEASE_MS", 20_000, &mut errors)),
        };
        if let Err(problem) = reconcile.validate() {
            errors.push(problem);
        }
        let log_format = match var("BP_LOG_FORMAT").as_deref() {
            None | Some("text") => LogFormat::Text,
            Some("json") => LogFormat::Json,
            Some(other) => {
                errors.push(format!("BP_LOG_FORMAT must be text or json, not {other:?}"));
                LogFormat::Text
            }
        };
        let config = Self {
            database_url: Secret(database_url),
            provider_url,
            concurrency,
            db_max_connections,
            poll_idle_max: Duration::from_millis(number(&var, "BP_POLL_MAX_MS", 200, &mut errors)),
            db_call_timeout: Duration::from_millis(number(
                &var,
                "BP_DB_CALL_TIMEOUT_MS",
                5_000,
                &mut errors,
            )),
            reconcile,
            shutdown_grace: Duration::from_millis(number(
                &var,
                "BP_SHUTDOWN_GRACE_MS",
                10_000,
                &mut errors,
            )),
            log_filter: var("BP_LOG").unwrap_or_else(|| "info".to_owned()),
            log_format,
        };
        if config.db_call_timeout.is_zero()
            || config.db_call_timeout.saturating_mul(2) > config.reconcile.lease
        {
            errors.push(
                "BP_DB_CALL_TIMEOUT_MS must be positive and at most half of BP_LEASE_MS".to_owned(),
            );
        }
        if errors.is_empty() {
            Ok(config)
        } else {
            Err(errors.join("; "))
        }
    }
}
