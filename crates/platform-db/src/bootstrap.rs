//! Roles and databases, created by an administrator before the first migration (F1).
//!
//! - `platform_owner` owns the schema and runs migrations (`platform-dbctl migrate`).
//! - `platform_app` is what the server and the worker run as: `NOSUPERUSER`, not the
//!   owner, no `CREATEROLE`/`CREATEDB`/`REPLICATION`/`BYPASSRLS`, no `TEMP` on the
//!   database, no `CREATE` on the schema, and on `operation_events` only `SELECT` and
//!   `INSERT` (migration 0002). [`crate::guard`] refuses to run as anything broader.

use std::str::FromStr;

use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, ConnectOptions, Connection, Executor};

use crate::DbError;

/// The schema owner, used only for migrations.
pub const OWNER_ROLE: &str = "platform_owner";
/// The runtime role of the server and the worker.
pub const RUNTIME_ROLE: &str = "platform_app";
/// Serializes concurrent bootstraps (parallel tests) on one cluster.
const BOOTSTRAP_LOCK: i64 = 7_316_002;

fn valid_identifier(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn valid_password(password: &str) -> bool {
    (8..=128).contains(&password.len())
        && password
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Creates (or updates) both roles and creates `database` owned by the owner role, with
/// `CONNECT` for the runtime role and nothing for `PUBLIC`. Idempotent.
pub async fn bootstrap(
    admin_url: &str,
    database: &str,
    owner_password: &str,
    runtime_password: &str,
) -> Result<(), DbError> {
    if !valid_identifier(database) {
        return Err(DbError::Corrupt(format!(
            "database name {database:?} is not a plain identifier"
        )));
    }
    if !(valid_password(owner_password) && valid_password(runtime_password)) {
        return Err(DbError::Corrupt(
            "role passwords must be 8 to 128 of [A-Za-z0-9_-]".to_owned(),
        ));
    }
    let mut admin = PgConnectOptions::from_str(admin_url)
        .map_err(DbError::Other)?
        .connect()
        .await?;
    admin
        .execute(AssertSqlSafe(format!(
            "SELECT pg_advisory_lock({BOOTSTRAP_LOCK})"
        )))
        .await?;
    let result = async {
        admin
            .execute(AssertSqlSafe(format!(
                "DO $$ BEGIN
                     IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{OWNER_ROLE}') THEN
                         CREATE ROLE {OWNER_ROLE};
                     END IF;
                     IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{RUNTIME_ROLE}') THEN
                         CREATE ROLE {RUNTIME_ROLE};
                     END IF;
                 END $$"
            )))
            .await?;
        let attributes = "LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS";
        admin
            .execute(AssertSqlSafe(format!(
                "ALTER ROLE {OWNER_ROLE} WITH {attributes} PASSWORD '{owner_password}'"
            )))
            .await?;
        admin
            .execute(AssertSqlSafe(format!(
                "ALTER ROLE {RUNTIME_ROLE} WITH {attributes} NOINHERIT PASSWORD '{runtime_password}'"
            )))
            .await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(database)
            .fetch_one(&mut admin)
            .await?;
        if !exists {
            admin.execute(AssertSqlSafe(format!("CREATE DATABASE {database} OWNER {OWNER_ROLE}"))).await?;
        }
        admin.execute(AssertSqlSafe(format!("REVOKE ALL ON DATABASE {database} FROM PUBLIC"))).await?;
        admin
            .execute(AssertSqlSafe(format!("GRANT CONNECT ON DATABASE {database} TO {RUNTIME_ROLE}")))
            .await?;
        Ok::<(), DbError>(())
    }
    .await;
    let _ = admin
        .execute(AssertSqlSafe(format!(
            "SELECT pg_advisory_unlock({BOOTSTRAP_LOCK})"
        )))
        .await;
    admin.close().await.ok();
    result
}

/// Drops `database` (forcing out its connections).
pub async fn drop_database(admin_url: &str, database: &str) -> Result<(), DbError> {
    if !valid_identifier(database) {
        return Err(DbError::Corrupt(format!(
            "database name {database:?} is not a plain identifier"
        )));
    }
    let mut admin = PgConnectOptions::from_str(admin_url)
        .map_err(DbError::Other)?
        .connect()
        .await?;
    admin
        .execute(AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {database} WITH (FORCE)"
        )))
        .await?;
    admin.close().await.ok();
    Ok(())
}

/// `url` with its user, password and database replaced (for building the owner and
/// runtime URLs from an administrator's URL).
pub fn role_url(url: &str, role: &str, password: &str, database: &str) -> Result<String, DbError> {
    let bad = || {
        DbError::Corrupt("database URL must look like postgres://user@host[:port]/db".to_owned())
    };
    let (scheme, rest) = url.split_once("://").ok_or_else(bad)?;
    let (authority, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let query = tail.split_once('?').map(|(_, q)| q);
    let mut out = format!("{scheme}://{role}:{password}@{host}/{database}");
    if let Some(query) = query {
        out.push('?');
        out.push_str(query);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_urls_replace_user_password_and_database() {
        assert_eq!(
            role_url(
                "postgres://platform:secret@127.0.0.1:61432/platform",
                RUNTIME_ROLE,
                "pw-12345",
                "bp_x"
            )
            .unwrap(),
            "postgres://platform_app:pw-12345@127.0.0.1:61432/bp_x"
        );
        assert_eq!(
            role_url(
                "postgresql://u@db.local/x?sslmode=disable",
                OWNER_ROLE,
                "pw-12345",
                "y"
            )
            .unwrap(),
            "postgresql://platform_owner:pw-12345@db.local/y?sslmode=disable"
        );
        assert!(role_url("not a url", OWNER_ROLE, "pw-12345", "y").is_err());
    }

    #[test]
    fn names_and_passwords_are_plain() {
        assert!(valid_identifier("bp_test_0190"));
        assert!(!valid_identifier("bp; DROP"));
        assert!(valid_password("platform-app-dev"));
        assert!(!valid_password("short"));
        assert!(!valid_password("quote'here-123"));
    }
}
