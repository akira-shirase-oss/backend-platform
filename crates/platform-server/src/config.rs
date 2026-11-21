//! Configuration from the environment: typed, validated once, every problem reported.
//! There is deliberately no provider setting here (T1).

use std::net::SocketAddr;
use std::time::Duration;

/// A secret that never prints.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// Wraps a value.
    pub fn new(value: String) -> Self {
        Self(value)
    }

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

/// Shortest admin token accepted.
pub const MIN_ADMIN_TOKEN_LEN: usize = 32;

/// The server's configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Listen address (default `127.0.0.1:61400`).
    pub listen: SocketAddr,
    /// The control database.
    pub database_url: Secret,
    /// The bootstrap admin bearer token.
    pub admin_token: Secret,
    /// Largest request body.
    pub max_body_bytes: usize,
    /// Deadline for a response.
    pub request_timeout: Duration,
    /// Most concurrent `/v1` requests before shedding with 503.
    pub max_in_flight: usize,
    /// Pool size.
    pub db_max_connections: u32,
    /// How long shutdown drains.
    pub shutdown_grace: Duration,
    /// Log filter.
    pub log_filter: String,
    /// Log format.
    pub log_format: LogFormat,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 61400)),
            database_url: Secret(String::new()),
            admin_token: Secret(String::new()),
            max_body_bytes: 16 * 1024,
            request_timeout: Duration::from_millis(2_000),
            // Four times the pool: beyond that a request would only queue for a
            // connection, so it is shed at once instead (T10, measured 2026-11-16).
            max_in_flight: 128,
            db_max_connections: 32,
            shutdown_grace: Duration::from_millis(10_000),
            log_filter: "info".to_owned(),
            log_format: LogFormat::Text,
        }
    }
}

fn parse<T: std::str::FromStr>(
    var: &dyn Fn(&str) -> Option<String>,
    name: &str,
    default: T,
    errors: &mut Vec<String>,
) -> T {
    match var(name) {
        None => default,
        Some(value) => value.parse().unwrap_or_else(|_| {
            errors.push(format!("{name} has an invalid value"));
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
        // ATTACK(config-default)
        let defaults = Self::default();
        let mut errors = Vec::new();
        let database_url = var("BP_DATABASE_URL").unwrap_or_else(|| {
            errors.push("BP_DATABASE_URL is required".to_owned());
            String::new()
        });
        let admin_token = var("BP_ADMIN_TOKEN").unwrap_or_default();
        // ATTACK(admin-token-len)
        if admin_token.len() < MIN_ADMIN_TOKEN_LEN
            || !admin_token.bytes().all(|b| b.is_ascii_graphic())
        {
            errors.push(format!(
                "BP_ADMIN_TOKEN is required: at least {MIN_ADMIN_TOKEN_LEN} printable characters"
            ));
        }
        let log_format = match var("BP_LOG_FORMAT").as_deref() {
            None | Some("text") => LogFormat::Text,
            Some("json") => LogFormat::Json,
            Some(_) => {
                errors.push("BP_LOG_FORMAT must be text or json".to_owned());
                LogFormat::Text
            }
        };
        let config = Self {
            listen: parse(&var, "BP_LISTEN", defaults.listen, &mut errors),
            database_url: Secret(database_url),
            admin_token: Secret(admin_token),
            max_body_bytes: parse(
                &var,
                "BP_MAX_BODY_BYTES",
                defaults.max_body_bytes,
                &mut errors,
            ),
            request_timeout: Duration::from_millis(parse(
                &var,
                "BP_REQUEST_TIMEOUT_MS",
                2_000,
                &mut errors,
            )),
            max_in_flight: parse(
                &var,
                "BP_MAX_IN_FLIGHT",
                defaults.max_in_flight,
                &mut errors,
            ),
            db_max_connections: parse(
                &var,
                "BP_DB_MAX_CONNECTIONS",
                defaults.db_max_connections,
                &mut errors,
            ),
            shutdown_grace: Duration::from_millis(parse(
                &var,
                "BP_SHUTDOWN_GRACE_MS",
                10_000,
                &mut errors,
            )),
            log_filter: var("BP_LOG").unwrap_or(defaults.log_filter),
            log_format,
        };
        if !(1..=1 << 20).contains(&config.max_body_bytes) {
            errors.push("BP_MAX_BODY_BYTES must be 1 to 1048576".to_owned());
        }
        if config.max_in_flight == 0 {
            errors.push("BP_MAX_IN_FLIGHT must be at least 1".to_owned());
        }
        if errors.is_empty() {
            Ok(config)
        } else {
            Err(errors.join("; "))
        }
    }
}
