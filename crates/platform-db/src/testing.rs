//! Test support: one throwaway, migrated database per test, set up exactly like
//! production — created by the administrator, owned and migrated by the owner role, used
//! through the runtime role.
//!
//! `BP_TEST_DATABASE_URL` names a PostgreSQL role allowed to create roles and databases
//! (the local stack's administrator: `scripts/stack.sh env` prints it). Tests that need a
//! database fail loudly without it rather than pass silently.
#![expect(
    clippy::expect_used,
    reason = "test support: a setup failure must abort the test with its cause"
)]

use std::str::FromStr;
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::{AssertSqlSafe, ConnectOptions, Connection, Executor};
use tokio::sync::{Semaphore, SemaphorePermit};
use uuid::Uuid;

use crate::bootstrap::{OWNER_ROLE, RUNTIME_ROLE, bootstrap, role_url};

/// The variable naming the administrator's connection.
pub const ENV: &str = "BP_TEST_DATABASE_URL";
/// Development password of the owner role (public, like the stack's).
pub const OWNER_PASSWORD: &str = "platform-owner-dev";
/// Development password of the runtime role (public, like the stack's).
pub const RUNTIME_PASSWORD: &str = "platform-app-dev";

/// Which role a connection uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The administrator (a superuser in the local stack).
    Admin,
    /// The schema owner.
    Owner,
    /// The runtime role.
    Runtime,
}

/// How many test databases the process may hold at once: creating roles and databases
/// serializes on a cluster-wide advisory lock and every extra pool is more connections
/// on a loaded server — tests wait their turn rather than pile up and time out.
static PERMITS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(4));

/// A process-wide runtime for the best-effort drop in [`TestDb::drop`]: each
/// `#[tokio::test]` runtime dies with its test, so work spawned there can be aborted
/// before the database is actually dropped. Multi-threaded so it drives itself.
static CLEANUP_RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn cleanup_rt() -> &'static tokio::runtime::Runtime {
    CLEANUP_RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("building the test-database cleanup runtime")
    })
}

/// A freshly created and migrated database, dropped by [`TestDb::cleanup`] — or, when a
/// test forgets to call it, best-effort by `Drop` on [`CLEANUP_RT`].
#[derive(Debug)]
pub struct TestDb {
    /// A pool on the new database, as the runtime role.
    pub pool: PgPool,
    admin_url: String,
    name: String,
    cleaned: bool,
    _permit: SemaphorePermit<'static>,
}

impl TestDb {
    /// Creates `bp_test_<uuid>`, migrates it as the owner, connects as the runtime role.
    ///
    /// # Panics
    ///
    /// When `BP_TEST_DATABASE_URL` is unset or the server is unreachable: a database test
    /// that cannot run must fail, not pass.
    pub async fn new() -> Self {
        let admin_url = std::env::var(ENV).unwrap_or_else(|_| {
            panic!("{ENV} is not set; start the stack with `scripts/stack.sh up` and export it")
        });
        let name = format!("bp_test_{}", Uuid::now_v7().simple());
        let permit = PERMITS
            .acquire()
            .await
            .expect("the test-database semaphore never closes");
        bootstrap(&admin_url, &name, OWNER_PASSWORD, RUNTIME_PASSWORD)
            .await
            .expect("bootstrapping the test database and roles");
        let mut db = Self {
            pool: PgPoolOptions::new().connect_lazy_with(
                PgConnectOptions::from_str(&admin_url)
                    .expect("invariant: BP_TEST_DATABASE_URL parses"),
            ),
            admin_url,
            name,
            cleaned: false,
            _permit: permit,
        };
        let owner = db.connect_as(Role::Owner).await;
        crate::MIGRATOR
            .run(&owner)
            .await
            .expect("migrating the test database as the owner");
        owner.close().await;
        let pool = db.connect_as(Role::Runtime).await;
        db.pool = pool;
        db
    }

    /// The URL for `role` on this database.
    pub fn url_as(&self, role: Role) -> String {
        match role {
            Role::Admin => {
                let (base, query) = match self.admin_url.split_once('?') {
                    Some((base, query)) => (base, Some(query)),
                    None => (self.admin_url.as_str(), None),
                };
                let prefix = base.rsplit_once('/').map_or(base, |(prefix, _)| prefix);
                match query {
                    Some(query) => format!("{prefix}/{}?{query}", self.name),
                    None => format!("{prefix}/{}", self.name),
                }
            }
            Role::Owner => role_url(&self.admin_url, OWNER_ROLE, OWNER_PASSWORD, &self.name)
                .expect("invariant: BP_TEST_DATABASE_URL is a postgres URL"),
            Role::Runtime => role_url(&self.admin_url, RUNTIME_ROLE, RUNTIME_PASSWORD, &self.name)
                .expect("invariant: BP_TEST_DATABASE_URL is a postgres URL"),
        }
    }

    /// The runtime role's URL (for child processes).
    pub fn url(&self) -> String {
        self.url_as(Role::Runtime)
    }

    /// A small pool on this database as `role`: two connections, and an acquire timeout
    /// that is a liveness bound only — a loaded machine must wait, never time a test out.
    pub async fn connect_as(&self, role: Role) -> PgPool {
        let options =
            crate::pool::connect_options(&self.url_as(role), &crate::PoolSettings::default())
                .expect("invariant: test url parses");
        PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(60))
            .connect_with(options)
            .await
            .expect("connecting to the test database")
    }

    /// Drops the database (forcing out any connection a test or child left behind).
    pub async fn cleanup(mut self) {
        self.cleaned = true;
        drop_database(&self.pool, &self.admin_url, &self.name).await;
    }
}

/// Closes `pool`, then drops `name` as the administrator — the body of both
/// [`TestDb::cleanup`] and the `Drop` fallback.
async fn drop_database(pool: &PgPool, admin_url: &str, name: &str) {
    let _ = tokio::time::timeout(Duration::from_secs(10), pool.close()).await;
    if let Ok(mut admin) = PgConnectOptions::from_str(admin_url)
        .expect("invariant: BP_TEST_DATABASE_URL parses")
        .connect()
        .await
    {
        let _ = admin
            .execute(AssertSqlSafe(format!(
                "DROP DATABASE IF EXISTS {name} WITH (FORCE)"
            )))
            .await;
        admin.close().await.ok();
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        // Tests should call cleanup(); this is the net for the ones that cannot — an
        // attack `Instance` is simply dropped when the test ends, and each `#[tokio::test]`
        // runtime dies too early to drive spawned work, so the drop runs on CLEANUP_RT.
        if self.cleaned {
            return;
        }
        let pool = self.pool.clone();
        let admin_url = self.admin_url.clone();
        let name = self.name.clone();
        cleanup_rt().spawn(async move {
            drop_database(&pool, &admin_url, &name).await;
        });
    }
}
