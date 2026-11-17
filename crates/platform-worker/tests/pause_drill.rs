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

/// Pauses once, between the lookup and the create of step 0.
struct PauseAfterLookup {
    pause: Duration,
    done: AtomicBool,
}

impl Hooks for PauseAfterLookup {
    async fn at(&self, point: Point, position: u16, _attempt: u16) -> Result<(), Halted> {
        if point == Point::AfterLookup && position == 0 && !self.done.swap(true, Ordering::SeqCst) {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_worker_paused_past_its_lease_does_not_call_the_provider() {
    let db = TestDb::new().await;
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
    let fake = std::sync::Arc::new(FakeProvider::new(FakeOptions {
        record_calls: true,
        ..FakeOptions::default()
    }));
    let log = std::sync::Arc::new(PgLog::new(db.pool.clone()));

    // Worker A: a 400 ms lease, then a 1.5 s pause after its first lookup.
    let short = config(Duration::from_millis(400));
    let claimed = log
        .claim(Uuid::now_v7(), short.lease)
        .await
        .unwrap()
        .unwrap();
    let (a_log, a_fake) = (log.clone(), fake.clone());
    let a = tokio::spawn(async move {
        let hooks = PauseAfterLookup {
            pause: Duration::from_millis(1_500),
            done: AtomicBool::new(false),
        };
        run_claimed(&*a_log, &*a_fake, &hooks, &short, claimed, &mut 1).await
    });

    // Worker B: takes over once A's lease has expired, and finishes.
    let b_log = log.clone();
    let b_fake = fake.clone();
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
    let a_outcome = a.await.unwrap();
    assert!(b_outcome.is_ok(), "{b_outcome:?}");
    assert!(a_outcome.is_err(), "A must stop: its lease is gone");

    let view = platform_db::log::get(&db.pool, id).await.unwrap().unwrap();
    assert_eq!(view.status, "succeeded");
    let a_creates_after_pause = fake
        .calls()
        .iter()
        .filter(|c| c.call == CallKind::Create)
        .count();
    check(&plan, &fake, None).unwrap_or_else(|e| panic!("{e} ({a_creates_after_pause} creates)"));
    db.cleanup().await;
}
