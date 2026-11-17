//! Every operation-log call has a client-side deadline (F12). A connection that goes
//! half-open mid-statement is invisible to the server's `statement_timeout` (the server
//! never sees the query), so without a deadline the reconcile task would wait forever.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test: a panic is the failure report"
)]

use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use platform_db::testing::TestDb;
use platform_worker::{OperationLog, PgLog};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

/// Forwards bytes until `frozen`, then silently drops them: a half-open connection.
async fn proxy(target: SocketAddr, frozen: Arc<AtomicBool>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let server = TcpStream::connect(target).await.unwrap();
            let (cr, cw) = client.into_split();
            let (sr, sw) = server.into_split();
            tokio::spawn(pipe(cr, sw, frozen.clone()));
            tokio::spawn(pipe(sr, cw, frozen.clone()));
        }
    });
    addr
}

async fn pipe(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    frozen: Arc<AtomicBool>,
) {
    let mut buf = vec![0_u8; 16 * 1024];
    while let Ok(n) = from.read(&mut buf).await {
        if n == 0 {
            break;
        }
        if !frozen.load(Ordering::SeqCst) && to.write_all(&buf[..n]).await.is_err() {
            break;
        }
    }
}

#[tokio::test]
async fn a_half_open_control_database_cannot_stall_the_worker() {
    let db = TestDb::new().await;
    let real = PgConnectOptions::from_str(&db.url()).unwrap();
    let target: SocketAddr = format!("{}:{}", real.get_host(), real.get_port())
        .parse()
        .unwrap();
    let frozen = Arc::new(AtomicBool::new(false));
    let through = proxy(target, frozen.clone()).await;
    // One pooled connection, handed out without a ping (as one that dies between the
    // ping and the query would be).
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect_with(real.clone().host("127.0.0.1").port(through.port()))
        .await
        .unwrap();
    let log = PgLog::with_deadline(pool, Duration::from_millis(500));
    assert!(
        log.claim(Uuid::nil(), Duration::from_secs(1))
            .await
            .unwrap()
            .is_none()
    );

    frozen.store(true, Ordering::SeqCst);
    let started = Instant::now();
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        log.claim(Uuid::nil(), Duration::from_secs(1)),
    )
    .await
    .expect("the claim hung on a half-open connection: no client-side deadline");
    assert!(
        outcome.is_err(),
        "a stalled database must surface as an error"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    db.cleanup().await;
}
