//! Shared fixtures: the full stack on `127.0.0.1:0` — real TCP, real middleware,
//! mail through the real SMTP adapter into a fake sink.
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

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use starter_accounts::mail::SmtpRelay;
use starter_accounts::testing::SmtpSink;
use starter_accounts::{Accounts, Settings, store, tokens};
use starter_db::testing::TestDb;
use starter_http::stack::{ErrorWindow, StackLimits};
use starter_server::health::Health;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

pub const PASSWORD: &str = "correct horse battery staple";

/// A running instance.
pub struct Instance {
    /// `http://127.0.0.1:<port>` — no path.
    pub base: String,
    /// The mail sink attacks can inspect.
    pub sink: SmtpSink,
    /// The database (invites are seeded through it).
    pub db: TestDb,
    /// The kit itself, for checks that must call below the HTTP layer.
    pub accounts: Accounts,
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
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn addr(&self) -> &str {
        self.base.strip_prefix("http://").unwrap_or(&self.base)
    }

    /// Creates an invite row and returns the code to present.
    pub async fn invite(&self) -> String {
        let code = tokens::new_invite().unwrap();
        store::create_invites(
            &self.db.pool,
            &[tokens::digest(&code)],
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
        code
    }

    /// Sign-up → verify link from the sink → sign-in; returns the session cookie value.
    pub async fn verified_session(&self, email: &str) -> String {
        let client = reqwest::Client::new();
        let invite = self.invite().await;
        let up = client
            .post(self.url("/v1/signup"))
            .json(&json!({ "invite": invite, "email": email, "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(up.status().as_u16(), 201);
        let token = self.sink.latest_to(email).await.unwrap().token().unwrap();
        let verified = client
            .post(self.url("/v1/verify"))
            .json(&json!({ "token": token }))
            .send()
            .await
            .unwrap();
        assert_eq!(verified.status().as_u16(), 204);
        let signin = client
            .post(self.url("/v1/session"))
            .json(&json!({ "email": email, "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(signin.status().as_u16(), 200);
        cookie_value(signin.headers(), "starter_session").unwrap()
    }
}

/// Extracts `name`'s value from a Set-Cookie header (`name=value; ...`).
pub fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| {
            c.split(';')
                .next()?
                .split_once('=')
                .and_then(|(n, v)| (n.trim() == name).then(|| v.to_owned()))
        })
}

/// Spawns the stack with default limits.
pub async fn spawn() -> Instance {
    spawn_with(|_| {}).await
}

/// Spawns with tuned settings/limits.
pub async fn spawn_with(tune: impl FnOnce(&mut (Settings, StackLimits))) -> Instance {
    let db = TestDb::new().await;
    let sink = SmtpSink::start("127.0.0.1:0", None).await.unwrap();
    let mut parts = (
        Settings {
            public_url: "https://app.example.test".to_owned(),
            cookie_secure: false,
            ..Settings::default()
        },
        StackLimits::default(),
    );
    tune(&mut parts);
    let (settings, limits) = parts;
    let accounts = Accounts::new(
        db.pool.clone(),
        settings,
        Arc::new(SmtpRelay::new(sink.addr).unwrap()),
    )
    .await
    .unwrap();
    // The real production composition — starter_server::app with the health
    // routes and every app-level layer — so guards applied outside /v1 take the
    // attack inputs too. 64 mirrors STARTER_MAX_IN_FLIGHT's documented default.
    let errors = Arc::new(ErrorWindow::default());
    let app = starter_server::app(starter_server::AppState {
        accounts: accounts.clone(),
        health: Health::new(
            db.pool.clone(),
            Duration::from_millis(500),
            errors.clone(),
            50,
            20,
        ),
        errors,
        stack: limits,
        max_in_flight: 64,
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Instance {
        base: format!("http://{addr}"),
        sink,
        db,
        accounts,
        server,
    }
}

/// Spawns behind the production connection loop (`serve::serve`), so the bounds under
/// test — the header-read deadline, the head buffer cap, connection limits — are the
/// shipped ones, not `axum::serve`'s.
pub async fn spawn_serve() -> Instance {
    let db = TestDb::new().await;
    let sink = SmtpSink::start("127.0.0.1:0", None).await.unwrap();
    let settings = Settings {
        public_url: "https://app.example.test".to_owned(),
        cookie_secure: false,
        ..Settings::default()
    };
    let accounts = Accounts::new(
        db.pool.clone(),
        settings,
        Arc::new(SmtpRelay::new(sink.addr).unwrap()),
    )
    .await
    .unwrap();
    let errors = Arc::new(ErrorWindow::default());
    let app = starter_server::app(starter_server::AppState {
        accounts: accounts.clone(),
        health: Health::new(
            db.pool.clone(),
            Duration::from_millis(500),
            errors.clone(),
            50,
            20,
        ),
        errors,
        stack: StackLimits::default(),
        max_in_flight: 64,
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        starter_http::serve::serve(
            listener,
            app,
            starter_http::serve::ConnectionLimits::default(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    });
    Instance {
        base: format!("http://{addr}"),
        sink,
        db,
        accounts,
        server,
    }
}

/// Writes `bytes` verbatim and returns the raw response (up to `deadline`).
pub async fn raw(instance: &Instance, bytes: &[u8], deadline: Duration) -> String {
    let mut stream = TcpStream::connect(instance.addr()).await.unwrap();
    stream.write_all(bytes).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(deadline, stream.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

/// The status code of a raw HTTP response.
pub fn raw_status(response: &str) -> u16 {
    response
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0)
}

/// JSON body as a Value, for asserting refusal envelopes.
pub async fn body_json(r: reqwest::Response) -> Value {
    r.json().await.unwrap_or(json!({}))
}
