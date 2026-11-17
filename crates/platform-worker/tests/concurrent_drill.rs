//! Concurrent workers on one PostgreSQL log (F14): three workers of four tasks each, 40
//! projects with three operations each, provider latency so that calls really overlap,
//! and random provider faults. The provider de-duplicates by idempotency key, so a late
//! create of one step cannot make a duplicate, but two *operations* of one project carry
//! different keys: only the per-project claim order keeps them from racing on the same
//! names. Pass: every operation succeeds, and the provider holds exactly the plan.
#![allow(clippy::unwrap_used, reason = "drill: a panic is the failure report")]

use std::sync::Arc;
use std::time::Duration;

use platform_db::testing::TestDb;
use platform_domain::{DesiredState, LogicalName, OperationId, Plan, ProjectId, RetryPolicy};
use platform_fake_provider::{FakeOptions, FakeProvider, FaultConfig, RandomFaults};
use platform_worker::worker::{WorkerConfig, run};
use platform_worker::{NoHooks, PgLog, ReconcileConfig};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const PROJECTS: usize = 40;
const OPERATIONS_PER_PROJECT: usize = 3;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_workers_converge_on_exactly_the_plan() {
    let db = TestDb::new().await;
    let desired = DesiredState {
        databases: vec![LogicalName::parse("main").unwrap()],
        buckets: vec![LogicalName::parse("files").unwrap()],
        identity: None,
    };
    let mut expected = 0;
    for p in 0..PROJECTS {
        let project = ProjectId::parse(&format!("conc{p:03}")).unwrap();
        let plan = Plan::build(&project, &desired).unwrap();
        expected += plan.resources.len();
        for _ in 0..OPERATIONS_PER_PROJECT {
            platform_db::put_desired(
                &db.pool,
                &project,
                &desired,
                &plan,
                None,
                OperationId(Uuid::now_v7()),
            )
            .await
            .unwrap();
        }
    }
    let fake = Arc::new(FakeProvider::new(FakeOptions {
        honor_idempotency_keys: true,
        latency: Duration::from_millis(3),
        faults: FaultConfig {
            rules: Vec::new(),
            random: Some(RandomFaults {
                seed: 14,
                per_million: 80_000,
                max: None,
            }),
        },
        ..FakeOptions::default()
    }));
    let log = Arc::new(PgLog::new(db.pool.clone()));
    let reconcile = ReconcileConfig {
        policy: RetryPolicy {
            max_attempts: 8,
            base: Duration::from_millis(2),
            cap: Duration::from_millis(20),
        },
        call_timeout: Duration::from_millis(500),
        lease: Duration::from_secs(10),
    };
    let cancel = CancellationToken::new();
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..3 {
        let config = WorkerConfig {
            id: Uuid::now_v7(),
            concurrency: 4,
            poll_idle_max: Duration::from_millis(20),
            reconcile,
        };
        workers.spawn(run(
            log.clone(),
            fake.clone(),
            Arc::new(NoHooks),
            config,
            cancel.clone(),
        ));
    }
    let total = i64::try_from(PROJECTS * OPERATIONS_PER_PROJECT).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let counts = loop {
        let counts = platform_db::log::counts(&db.pool).await.unwrap();
        if counts.succeeded + counts.partial == total {
            break counts;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "not drained: {counts:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    cancel.cancel();
    while workers.join_next().await.is_some() {}
    let inventory = fake.inventory();
    assert_eq!(
        inventory.duplicates,
        vec![],
        "concurrent operations raced on a name"
    );
    assert_eq!(counts.succeeded, total, "{counts:?}");
    assert_eq!(inventory.resources, expected);
    assert!(
        inventory.faults_injected > 20,
        "faults must have fired: {inventory:?}"
    );
    db.cleanup().await;
}
