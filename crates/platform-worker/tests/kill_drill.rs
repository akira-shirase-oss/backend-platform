//! Drill `partial-provision`, process level: the real `platform-worker` binary, the control
//! database in PostgreSQL, the fake provider over HTTP.
//!
//! Kill cases: the worker is parked at a fault point — after the attempt is committed,
//! after the lookup, or after the create took effect but before it was recorded — at every
//! step of the starter plan, and then killed with SIGKILL. A fresh worker must resume the
//! same operation after the lease expires and converge on exactly the planned resources.
//!
//! Provider cases: no kill; the provider fails a call before any effect, loses a create's
//! reply, or holds the reply past the worker's call deadline after taking effect — for
//! every resource kind. The worker alone must converge.
//!
//! Pass for every case: the provider holds exactly one resource per planned resource and
//! nothing else; every recorded id is the provider's; no create was sent without a delivered
//! "absent" lookup immediately before it. Needs `BP_TEST_DATABASE_URL` and
//! `--features fault-injection`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "drill: a panic is the failure report"
)]

mod common;

use std::os::unix::process::ExitStatusExt as _;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use common::check;
use platform_db::logcheck::{Captured, assert_no_leaks};
use platform_db::testing::TestDb;
use platform_domain::{
    DesiredState, IdentitySpec, LogicalName, OperationId, Plan, ProjectId, ResourceKind,
};
use platform_fake_provider::server::{AppState, router};
use platform_fake_provider::{CallKind, FakeOptions, FakeProvider, Fault, FaultConfig, FaultRule};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::{Child, Command};
use uuid::Uuid;

const LEASE_MS: u64 = 1_500;
const CALL_TIMEOUT_MS: u64 = 200;
const PROVIDER_HANG: Duration = Duration::from_secs(2);

struct Case {
    db: TestDb,
    fake: Arc<FakeProvider>,
    provider_url: String,
    plan: Plan,
    operation: OperationId,
    server: tokio::task::JoinHandle<()>,
    /// Every worker's stderr, at debug level: the log-capture check reads it (F11).
    logs: Captured,
}

impl Case {
    async fn new(faults: FaultConfig) -> Self {
        Self::with(FakeOptions {
            faults,
            ..FakeOptions::default()
        })
        .await
    }

    async fn with(options: FakeOptions) -> Self {
        let db = TestDb::new().await;
        let fake = Arc::new(FakeProvider::new(FakeOptions {
            record_calls: true,
            ..options
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let provider_url = format!("http://{}", listener.local_addr().unwrap());
        let app = router(AppState {
            fake: fake.clone(),
            hang: PROVIDER_HANG,
        });
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let project = ProjectId::parse("starter").unwrap();
        let desired = DesiredState {
            databases: vec![LogicalName::parse("main").unwrap()],
            buckets: vec![LogicalName::parse("files").unwrap()],
            identity: Some(IdentitySpec {
                clients: vec![LogicalName::parse("web").unwrap()],
            }),
        };
        let plan = Plan::build(&project, &desired).unwrap();
        let operation = OperationId(Uuid::now_v7());
        platform_db::put_desired(&db.pool, &project, &desired, &plan, None, operation)
            .await
            .unwrap();
        Self {
            db,
            fake,
            provider_url,
            plan,
            operation,
            server,
            logs: Captured::default(),
        }
    }

    fn worker(&self, halt_at: Option<&str>) -> Child {
        let mut command = Command::new(env!("CARGO_BIN_EXE_platform-worker"));
        command
            .env("BP_DATABASE_URL", self.db.url())
            .env("BP_PROVIDER_URL", &self.provider_url)
            .env("BP_WORKER_CONCURRENCY", "1")
            .env("BP_LEASE_MS", LEASE_MS.to_string())
            .env("BP_CALL_TIMEOUT_MS", CALL_TIMEOUT_MS.to_string())
            .env("BP_DB_CALL_TIMEOUT_MS", "700")
            .env("BP_BACKOFF_BASE_MS", "10")
            .env("BP_BACKOFF_CAP_MS", "50")
            .env("BP_POLL_MAX_MS", "50")
            .env("BP_LOG", "debug")
            .env_remove("BP_HALT_AT")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(halt_at) = halt_at {
            command.env("BP_HALT_AT", halt_at);
        }
        let mut child = command.spawn().expect("spawning platform-worker");
        // Drain stderr into the capture, always: a full pipe would block the worker.
        let stderr = child.stderr.take().expect("invariant: stderr is piped");
        let logs = self.logs.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                logs.push(&line);
            }
        });
        child
    }

    async fn status(&self) -> platform_db::OperationSummary {
        platform_db::log::get(&self.db.pool, self.operation)
            .await
            .unwrap()
            .unwrap()
    }

    async fn wait_until_succeeded(&self, label: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let status = self.status().await;
            if status.status == "succeeded" {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{label}: not converged: {status:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The pass condition, provider side and control-DB side.
    async fn verify(&self, label: &str) {
        check(&self.plan, &self.fake, None).unwrap_or_else(|e| panic!("{label}: {e}"));
        let status = self.status().await;
        let held = self.fake.resources();
        for step in &status.steps {
            let on_provider: Vec<_> = held
                .iter()
                .filter(|r| r.name.as_str() == step.provider_name)
                .collect();
            assert_eq!(on_provider.len(), 1, "{label}: step {}", step.position);
            assert_eq!(
                step.resource_id.as_deref(),
                Some(on_provider[0].id.as_str()),
                "{label}: step {} id",
                step.position
            );
        }
        let observed: i64 = sqlx::query_scalar("SELECT count(*) FROM observed_resources")
            .fetch_one(&self.db.pool)
            .await
            .unwrap();
        assert_eq!(
            observed,
            i64::try_from(self.plan.resources.len()).unwrap(),
            "{label}: observed rows"
        );
    }

    async fn finish(self) {
        self.server.abort();
        // The workers were given database URLs with passwords: none may reach a log line.
        assert_no_leaks(&self.logs.text(), &[], "worker logs");
        self.db.cleanup().await;
    }
}

async fn wait_for_halt(logs: &Captured, label: &str) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(line) = logs
            .text()
            .lines()
            .find(|l| l.contains("bp-worker halted at"))
        {
            return line.to_owned();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{label}: worker never reached the halt point"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn sigkill(mut child: Child, label: &str) {
    child.start_kill().unwrap();
    let status = child.wait().await.unwrap();
    assert_eq!(
        status.signal(),
        Some(9),
        "{label}: the worker must die by SIGKILL"
    );
}

async fn kill_case(point: &'static str, position: u16, names: Names) -> String {
    let label = format!("kill -9 at {point}:{position} ({names:?} names)");
    let case = Case::with(names.options(FaultConfig::default())).await;
    let first = case.worker(Some(&format!("{point}:{position}")));
    let halted = wait_for_halt(&case.logs, &label).await;
    // While parked, the operation shows exactly where it is: never fabricated success.
    let before = case.status().await;
    if point != "after-finish" {
        assert_ne!(before.status, "succeeded", "{label}");
        let step = &before.steps[usize::from(position)];
        assert_eq!(step.status, "uncertain", "{label}: {before:?}");
        if point == "after-create" {
            let made = case
                .fake
                .resources()
                .iter()
                .any(|r| r.name.as_str() == step.provider_name);
            assert!(made, "{label}: the create took effect before the kill");
        }
    }
    sigkill(first, &label).await;
    let second = case.worker(None);
    case.wait_until_succeeded(&label).await;
    case.verify(&label).await;
    let creates = case
        .fake
        .calls()
        .iter()
        .filter(|c| c.call == CallKind::Create)
        .count();
    drop(second); // kill_on_drop
    case.finish().await;
    format!("{label}: {halted}; converged, {creates} creates for 5 resources")
}

/// What the provider promises about names (F6: the drill runs both plain and unique).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Names {
    Plain,
    Keys,
    Unique,
}

impl Names {
    fn options(self, faults: FaultConfig) -> FakeOptions {
        FakeOptions {
            faults,
            honor_idempotency_keys: self == Self::Keys,
            unique_names: self == Self::Unique,
            ..FakeOptions::default()
        }
    }
}

async fn provider_case(on: CallKind, kind: ResourceKind, fault: Fault, names: Names) -> String {
    let label = format!("{fault:?} on {on:?} of {kind} ({names:?} names)");
    let faults = FaultConfig {
        rules: vec![FaultRule {
            on,
            kind: Some(kind),
            nth: 1,
            fault,
        }],
        random: None,
    };
    let case = Case::with(names.options(faults)).await;
    let worker = case.worker(None);
    case.wait_until_succeeded(&label).await;
    case.verify(&label).await;
    let injected = case.fake.inventory().faults_injected;
    assert_eq!(injected, 1, "{label}: the fault fired");
    drop(worker);
    case.finish().await;
    format!("{label}: converged")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kill_dash_nine_at_every_step_converges_on_retry() {
    let mut cases = tokio::task::JoinSet::new();
    let gate = Arc::new(tokio::sync::Semaphore::new(6));
    for names in [Names::Plain, Names::Unique] {
        for position in 0..5_u16 {
            for point in ["after-begin", "after-lookup", "after-create"] {
                let gate = gate.clone();
                cases.spawn(async move {
                    let _permit = gate.acquire_owned().await.unwrap();
                    kill_case(point, position, names).await
                });
            }
        }
        let gate2 = gate.clone();
        cases.spawn(async move {
            let _permit = gate2.acquire_owned().await.unwrap();
            kill_case("after-finish", 0, names).await
        });
    }
    let mut report = cases.join_all().await;
    report.sort();
    for line in &report {
        eprintln!("{line}");
    }
    assert_eq!(report.len(), 32);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provider_failures_at_every_kind_converge() {
    let mut cases = tokio::task::JoinSet::new();
    let gate = Arc::new(tokio::sync::Semaphore::new(6));
    for (kind, names) in ResourceKind::ALL
        .into_iter()
        .flat_map(|k| [(k, Names::Plain), (k, Names::Unique)])
    {
        // A late create: plain names cannot be made duplicate-free (that case is reported,
        // see below); with keys it is replayed, with unique names it is adopted.
        let late_names = if names == Names::Plain {
            Names::Keys
        } else {
            Names::Unique
        };
        for (on, fault) in [
            (CallKind::Lookup, Fault::FailBefore),
            (CallKind::Create, Fault::FailBefore),
            (CallKind::Create, Fault::LoseReply),
            (CallKind::Create, Fault::Hang),
        ] {
            let gate = gate.clone();
            cases.spawn(async move {
                let _permit = gate.acquire_owned().await.unwrap();
                provider_case(on, kind, fault, names).await
            });
        }
        // A create that lands after the deadline, on a provider that de-duplicates by key:
        // converges on exactly the plan (F5).
        let gate = gate.clone();
        cases.spawn(async move {
            let _permit = gate.acquire_owned().await.unwrap();
            provider_case(CallKind::Create, kind, Fault::LateEffect, late_names).await
        });
    }
    let mut report = cases.join_all().await;
    report.sort();
    for line in &report {
        eprintln!("{line}");
    }
    assert_eq!(report.len(), 50);
}

/// A create that lands after the deadline on a provider with neither keys nor unique
/// names, over HTTP: the duplicate it makes is reported (`partial`, step blocked), and the
/// operation never reads `succeeded` (F5).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_create_over_http_is_reported_never_hidden() {
    let faults = FaultConfig {
        rules: vec![FaultRule {
            on: CallKind::Create,
            kind: Some(ResourceKind::Bucket),
            nth: 1,
            fault: Fault::LateEffect,
        }],
        random: None,
    };
    let case = Case::new(faults).await;
    let worker = case.worker(None);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let status = loop {
        let status = case.status().await;
        assert_ne!(
            status.status, "succeeded",
            "a duplicate was hidden behind a success"
        );
        if status.status == "partial" {
            break status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "not settled: {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let bucket = status.steps.iter().find(|s| s.kind == "bucket").unwrap();
    assert_eq!(bucket.status, "blocked");
    assert_eq!(case.fake.inventory().duplicates.len(), 1);
    drop(worker);
    case.finish().await;
}
