//! Shared fixtures: a `platform-server` bound on `127.0.0.1:0` and helpers for
//! malformed traffic that reqwest will not produce.
#![allow(
    dead_code,
    reason = "each integration-test crate uses a different subset"
)]
#![allow(
    unreachable_pub,
    reason = "a test module shared by several test binaries"
)]
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

use std::time::Duration;

use platform_db::testing::TestDb;
use platform_server::config::Secret;
use platform_server::{AppState, Config};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// The bootstrap token every attack knows only by failing against.
pub const TOKEN: &str = "attack-admin-token-0123456789abcdef";

/// A running instance: the real router, the real middleware, real sockets.
pub struct Instance {
    /// `http://127.0.0.1:<port>` — no path.
    pub base: String,
    _db: TestDb,
    server: JoinHandle<()>,
}

impl Drop for Instance {
    fn drop(&mut self) {
        // A dropped JoinHandle only detaches: abort the server so its pool closes and
        // the database's own Drop can drop it (FORCE) without waiting on stale clients.
        self.server.abort();
    }
}

impl Instance {
    /// A full URL for `path` (pass `/v1/...` or `/livez`).
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The socket address, for raw writes.
    pub fn addr(&self) -> &str {
        self.base.strip_prefix("http://").unwrap_or(&self.base)
    }
}

/// The production configuration shape with the attack token.
pub fn config() -> Config {
    Config {
        admin_token: Secret::new(TOKEN.to_owned()),
        ..Config::default()
    }
}

/// Spawns the server against a fresh migrated database.
pub async fn spawn() -> Instance {
    spawn_with(config()).await
}

/// Spawns with a tuned configuration (smaller limits, longer timeouts).
pub async fn spawn_with(config: Config) -> Instance {
    let db = TestDb::new().await;
    let app = platform_server::app(&config, AppState::new(&config, db.pool.clone()).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Instance {
        base: format!("http://{addr}"),
        _db: db,
        server,
    }
}

/// Writes `bytes` verbatim to `instance` and returns the raw response (up to `deadline`).
pub async fn raw(instance: &Instance, bytes: &[u8], deadline: Duration) -> String {
    let mut stream = TcpStream::connect(instance.addr()).await.unwrap();
    stream.write_all(bytes).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(deadline, stream.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

/// The status line's code from a raw HTTP response, e.g. `HTTP/1.1 413`.
pub fn raw_status(response: &str) -> u16 {
    response
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0)
}

/// Bearer-auth GET/PUT convenience for the well-formed attacks.
pub async fn authed(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
) -> reqwest::Response {
    client
        .request(method, url)
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
}
