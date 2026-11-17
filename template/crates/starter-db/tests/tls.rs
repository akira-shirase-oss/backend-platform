//! TLS to PostgreSQL, against a server whose certificate a private CA signed for
//! `localhost` only. `scripts/test-tls.sh` makes the CAs and the certificate, starts that
//! server and runs these (they are ignored in a plain `cargo test`, which has no such
//! server): `verify-full` with the right root connects over TLS; a wrong name, another CA or
//! the public roots alone are refused; `require` accepts any certificate, which is why
//! [`starter_db::parse_url`] refuses it off loopback.

use std::time::Duration;

use sqlx::{Connection as _, PgConnection};

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set: run scripts/test-tls.sh"))
}

/// The TLS server's URL: `host` with these query parameters.
fn url(host: &str, query: &str) -> String {
    let port = env("STARTER_TLS_TEST_PORT");
    format!("postgres://postgres:starter-tls-dev@{host}:{port}/postgres?{query}")
}

async fn connect(url: &str) -> Result<PgConnection, sqlx::Error> {
    let options = starter_db::parse_url(url).expect("a URL the transport rule accepts");
    tokio::time::timeout(
        Duration::from_secs(20),
        PgConnection::connect_with(&options),
    )
    .await
    .expect("the connection attempt finishes within 20 s")
}

async fn assert_tls(url: &str) {
    let mut conn = connect(url).await.unwrap_or_else(|e| panic!("{url}: {e}"));
    let ssl: bool = sqlx::query_scalar("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert!(ssl, "{url} connected without TLS");
    conn.close().await.unwrap();
}

async fn assert_refused(url: &str) {
    match connect(url).await {
        Ok(_) => panic!("{url} connected; the certificate should have been refused"),
        Err(sqlx::Error::Tls(_)) => {}
        Err(other) => panic!("{url}: refused, but not by TLS: {other}"),
    }
}

#[tokio::test]
#[ignore = "needs the TLS PostgreSQL that scripts/test-tls.sh starts"]
async fn verify_full_with_the_right_root_connects_over_tls() {
    let ca = env("STARTER_TLS_TEST_CA");
    assert_tls(&url(
        "localhost",
        &format!("sslmode=verify-full&sslrootcert={ca}"),
    ))
    .await;
}

#[tokio::test]
#[ignore = "needs the TLS PostgreSQL that scripts/test-tls.sh starts"]
async fn verify_full_refuses_a_name_the_certificate_does_not_carry() {
    // The certificate names `localhost` only; 127.0.0.1 is the same server under a name the
    // certificate does not carry, as a relay on the path would be.
    let ca = env("STARTER_TLS_TEST_CA");
    assert_refused(&url(
        "127.0.0.1",
        &format!("sslmode=verify-full&sslrootcert={ca}"),
    ))
    .await;
}

#[tokio::test]
#[ignore = "needs the TLS PostgreSQL that scripts/test-tls.sh starts"]
async fn verify_full_refuses_a_certificate_from_another_ca() {
    let other = env("STARTER_TLS_TEST_OTHER_CA");
    assert_refused(&url(
        "localhost",
        &format!("sslmode=verify-full&sslrootcert={other}"),
    ))
    .await;
    // With no root named, only the public roots are trusted, and they did not sign it.
    assert_refused(&url("localhost", "sslmode=verify-full")).await;
}

#[tokio::test]
#[ignore = "needs the TLS PostgreSQL that scripts/test-tls.sh starts"]
async fn require_accepts_any_certificate_so_it_is_refused_off_loopback() {
    // On loopback, `require` connects to this server with no root at all and under a name
    // the certificate does not carry: it encrypts but authenticates nothing...
    assert_tls(&url("127.0.0.1", "sslmode=require")).await;
    // ...so the same mode to any other host is refused before anything connects.
    let off = "postgres://postgres:starter-tls-dev@db.example.com:5432/postgres?sslmode=require";
    assert!(matches!(
        starter_db::parse_url(off),
        Err(starter_db::DbError::Transport(
            starter_db::TransportError::Unverified { mode: "require" }
        ))
    ));
}
