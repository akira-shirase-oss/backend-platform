//! A worker paused past its lease must not call the provider (F4). Fencing tokens protect
//! the log, not the provider: without a lease check before each provider call, a worker
//! that wakes after its successor took over would still send its create.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "drill: a panic is the failure report"
)]
#![allow(
    clippy::unused_async_trait_impl,
    reason = "the hook mirrors the async port"
)]

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::check;
use platform_db::testing::TestDb;
use platform_domain::{DesiredState, LogicalName, OperationId, Plan, ProjectId, RetryPolicy};
use platform_fake_provider::{CallKind, FakeOptions, FakeProvider};
use platform_worker::{
    Halted, Hooks, NoHooks, OperationLog, PgLog, Point, ReconcileConfig, run_claimed,
};
use uuid::Uuid;

/// Pauses once at `point` of step 0.
struct PauseOnce {
    point: Point,
    pause: Duration,
    done: AtomicBool,
}

impl Hooks for PauseOnce {
    async fn at(&self, point: Point, position: u16, _attempt: u16) -> Result<(), Halted> {
        if point == self.point && position == 0 && !self.done.swap(true, Ordering::SeqCst) {
            tokio::time::sleep(self.pause).await;
        }
        Ok(())
    }
}

fn config(lease: Duration) -> ReconcileConfig {
    ReconcileConfig {
        policy: RetryPolicy {
            max_attempts: 5,
            base: Duration::from_millis(10),
            cap: Duration::from_millis(20),
        },
        call_timeout: Duration::from_millis(100),
        lease,
    }
}

struct Outcome {
    fake: std::sync::Arc<FakeProvider>,
    plan: Plan,
    a: Result<platform_domain::OperationOutcome, platform_worker::RunError>,
}

/// Worker A (a 400 ms lease) pauses 1.5 s at `point`; worker B takes over and finishes.
async fn race(db: &TestDb, point: Point, options: FakeOptions) -> Outcome {
    let (logs, _capture) = platform_db::logcheck::capture();
    let project = ProjectId::parse("pause").unwrap();
    let desired = DesiredState {
        databases: vec![LogicalName::parse("main").unwrap()],
        ..DesiredState::default()
    };
    let plan = Plan::build(&project, &desired).unwrap();
    let id = OperationId(Uuid::now_v7());
    platform_db::put_desired(&db.pool, &project, &desired, &plan, None, id)
        .await
        .unwrap();
    let fake = std::sync::Arc::new(FakeProvider::new(options));
    let log = std::sync::Arc::new(PgLog::new(db.pool.clone()));
    let short = config(Duration::from_millis(400));
    let claimed = log
        .claim(Uuid::now_v7(), short.lease)
        .await
        .unwrap()
        .unwrap();
    let (a_log, a_fake) = (log.clone(), fake.clone());
    let a = tokio::spawn(async move {
        let hooks = PauseOnce {
            point,
            pause: Duration::from_millis(1_500),
            done: AtomicBool::new(false),
        };
        run_claimed(&*a_log, &*a_fake, &hooks, &short, claimed, &mut 1).await
    });
    let (b_log, b_fake) = (log.clone(), fake.clone());
    let b = tokio::spawn(async move {
        let successor = config(Duration::from_secs(10));
        loop {
            if let Some(claimed) = b_log.claim(Uuid::now_v7(), successor.lease).await.unwrap() {
                return run_claimed(&*b_log, &*b_fake, &NoHooks, &successor, claimed, &mut 2).await;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    let b_outcome = b.await.unwrap();
    assert!(b_outcome.is_ok(), "{b_outcome:?}");
    let a = a.await.unwrap();
    let view = platform_db::log::get(&db.pool, id).await.unwrap().unwrap();
    assert_eq!(view.status, "succeeded");
    platform_db::logcheck::assert_no_leaks(&logs.text(), &[], "drill logs");
    Outcome { fake, plan, a }
}

/// Paused before the lease check: A refuses to call the provider at all (F4).
#[tokio::test]
async fn a_worker_paused_past_its_lease_does_not_call_the_provider() {
    let db = TestDb::new().await;
    let out = race(
        &db,
        Point::AfterLookup,
        FakeOptions {
            record_calls: true,
            ..FakeOptions::default()
        },
    )
    .await;
    assert!(
        matches!(out.a, Err(platform_worker::RunError::LeaseExpired)),
        "A must stop: {:?}",
        out.a
    );
    let creates = out
        .fake
        .calls()
        .iter()
        .filter(|c| c.call == CallKind::Create)
        .count();
    check(&out.plan, &out.fake, None).unwrap_or_else(|e| panic!("{e} ({creates} creates)"));
    db.cleanup().await;
}

/// Paused after the lease check, before the create is sent (D-029's residual window): A's
/// stale create reaches the provider after B finished. A provider that de-duplicates by
/// idempotency key returns B's resource instead of making a second one.
#[tokio::test]
async fn a_stale_create_in_the_residual_window_is_deduplicated_by_its_key() {
    let db = TestDb::new().await;
    let options = FakeOptions {
        honor_idempotency_keys: true,
        ..FakeOptions::default()
    };
    let out = race(&db, Point::BeforeCreate, options).await;
    let inventory = out.fake.inventory();
    assert_eq!(
        inventory.duplicates,
        vec![],
        "the stale create made a second resource"
    );
    assert_eq!(inventory.resources, out.plan.resources.len());
    db.cleanup().await;
}
