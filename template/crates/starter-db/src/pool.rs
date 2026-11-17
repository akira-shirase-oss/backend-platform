//! The connection pool: bounded, with a deadline on acquiring a connection and on every
//! statement and lock wait, so a slow or stuck database becomes a fast refusal, never a
//! pile of stalled requests.
//!
//! Every database URL is parsed by [`parse_url`], which refuses TLS that would not
//! authenticate the server ([`TransportError`]) before anything connects.

use std::net::IpAddr;
use std::str::FromStr;
use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgSslMode};

use crate::DbError;
use crate::error::TransportError;

/// Pool limits and per-statement deadlines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolSettings {
    /// Most open connections.
    pub max_connections: u32,
    /// Longest wait for a free connection (and for a new connection to open).
    pub acquire_timeout: Duration,
    /// Longest any single statement may run (`statement_timeout`).
    pub statement_timeout: Duration,
    /// Longest any statement may wait for a lock (`lock_timeout`).
    pub lock_timeout: Duration,
}

impl Default for PoolSettings {
    fn default() -> Self {
        Self {
            max_connections: 16,
            acquire_timeout: Duration::from_millis(2_000),
            statement_timeout: Duration::from_millis(3_000),
            lock_timeout: Duration::from_millis(1_000),
        }
    }
}

/// Parses a database URL, refusing one whose connection would not authenticate the server:
/// off loopback (a TCP host other than `localhost`, `127.0.0.0/8` or `::1`, or a Unix
/// socket) only `sslmode=verify-full` is accepted, and `channel_binding=require`, which sqlx
/// ignores, is refused everywhere rather than silently dropped.
pub fn parse_url(url: &str) -> Result<PgConnectOptions, DbError> {
    let options = PgConnectOptions::from_str(url).map_err(DbError::Other)?;
    check_transport(&options, url)?;
    Ok(options)
}

/// The transport rule of [`parse_url`] on options already parsed from `url` (`url` for the
/// parameters sqlx drops while parsing).
pub fn check_transport(options: &PgConnectOptions, url: &str) -> Result<(), TransportError> {
    let query = url.split_once('?').map_or("", |(_, q)| q);
    let channel_binding = query
        .split('&')
        .filter_map(|p| p.split_once('='))
        .any(|(k, v)| {
            k.eq_ignore_ascii_case("channel_binding") && v.eq_ignore_ascii_case("require")
        });
    if channel_binding {
        return Err(TransportError::ChannelBinding);
    }
    if is_loopback(options) {
        return Ok(());
    }
    match options.get_ssl_mode() {
        PgSslMode::VerifyFull => Ok(()),
        PgSslMode::VerifyCa => Err(TransportError::VerifyCa),
        PgSslMode::Require => Err(TransportError::Unverified { mode: "require" }),
        PgSslMode::Prefer => Err(TransportError::Unverified { mode: "prefer" }),
        PgSslMode::Allow => Err(TransportError::Unverified { mode: "allow" }),
        PgSslMode::Disable => Err(TransportError::Unverified { mode: "disable" }),
    }
}

/// A Unix socket, `localhost`, or a loopback address: no network path to intercept.
fn is_loopback(options: &PgConnectOptions) -> bool {
    if options.get_socket().is_some() {
        return true;
    }
    let host = options.get_host();
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    bare.eq_ignore_ascii_case("localhost")
        || bare.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Parses `url` ([`parse_url`]) and applies the deadlines as session settings.
pub fn connect_options(url: &str, settings: &PoolSettings) -> Result<PgConnectOptions, DbError> {
    let options = parse_url(url)?;
    Ok(options.application_name("starter").options([
        (
            "statement_timeout",
            format!("{}ms", settings.statement_timeout.as_millis()),
        ),
        (
            "lock_timeout",
            format!("{}ms", settings.lock_timeout.as_millis()),
        ),
        // A connection left idle inside a transaction (a crashed caller) is cut off
        // instead of holding row locks for ever.
        ("idle_in_transaction_session_timeout", "10000ms".to_owned()),
    ]))
}

/// Builds the pool without connecting, so a process starts (and reports not ready) while
/// the database is down.
pub fn connect_lazy(url: &str, settings: &PoolSettings) -> Result<PgPool, DbError> {
    let options = connect_options(url, settings)?;
    Ok(PgPoolOptions::new()
        .max_connections(settings.max_connections)
        .acquire_timeout(settings.acquire_timeout)
        .idle_timeout(Some(Duration::from_secs(300)))
        .connect_lazy_with(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(url: &str) -> Option<TransportError> {
        match parse_url(url) {
            Ok(_) => None,
            Err(DbError::Transport(t)) => Some(t),
            Err(other) => panic!("{url}: {other}"),
        }
    }

    #[test]
    fn off_loopback_only_verify_full_passes() {
        let host = "postgres://u:p@ep-x.eu-central-1.aws.neon.tech:5432/app";
        assert_eq!(refusal(&format!("{host}?sslmode=verify-full")), None);
        assert_eq!(
            refusal(&format!(
                "{host}?sslmode=verify-full&sslrootcert=/etc/ca.pem"
            )),
            None
        );
        for (query, mode) in [
            ("?sslmode=require", "require"),
            ("?sslmode=prefer", "prefer"),
            ("", "prefer"),
            ("?sslmode=allow", "allow"),
            ("?sslmode=disable", "disable"),
        ] {
            assert_eq!(
                refusal(&format!("{host}{query}")),
                Some(TransportError::Unverified { mode }),
                "{query}"
            );
        }
        assert_eq!(
            refusal(&format!("{host}?sslmode=verify-ca&sslrootcert=/etc/ca.pem")),
            Some(TransportError::VerifyCa)
        );
        // An address that only looks local is not loopback.
        assert!(refusal("postgres://u@10.0.0.5/app?sslmode=require").is_some());
        assert!(refusal("postgres://u@[fd00::5]/app").is_some());
        assert!(refusal("postgres://u@localhost.example.com/app").is_some());
    }

    #[test]
    fn loopback_and_sockets_take_any_mode() {
        for url in [
            "postgres://u@127.0.0.1:61432/app",
            "postgres://u@127.0.0.9/app?sslmode=disable",
            "postgres://u@localhost/app?sslmode=require",
            "postgres://u@[::1]:5432/app",
            "postgres://u@%2Frun%2Fpostgresql/app",
            "postgres:///app?host=/run/postgresql",
        ] {
            assert_eq!(refusal(url), None, "{url}");
        }
    }

    #[test]
    fn channel_binding_require_is_refused_not_ignored() {
        assert_eq!(
            refusal("postgres://u@ep-x.neon.tech/app?sslmode=verify-full&channel_binding=require"),
            Some(TransportError::ChannelBinding)
        );
        assert_eq!(
            refusal("postgres://u@127.0.0.1/app?channel_binding=require"),
            Some(TransportError::ChannelBinding)
        );
        assert_eq!(
            refusal("postgres://u@ep-x.neon.tech/app?sslmode=verify-full&channel_binding=prefer"),
            None
        );
    }

    #[test]
    fn the_refusal_names_the_fix_and_no_secret() {
        let text = parse_url("postgres://u:hunter2-secret@db.example.com/app?sslmode=require")
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(text.contains("sslmode=verify-full"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
    }
}
