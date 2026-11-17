//! The control store against a real PostgreSQL (one throwaway database per test).
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

use std::time::Duration;

use platform_db::testing::TestDb;
use platform_db::{AdmitError, MAX_IN_FLIGHT_PER_PROJECT, RetryError, log, put_desired};
use platform_domain::{
    DesiredState, ErrorClass, LogicalName, OperationId, OperationState, Plan, ProjectId,
    ProviderResourceId, RetryPolicy, StepState,
};
use uuid::Uuid;

fn desired(dbs: &[&str]) -> DesiredState {
    DesiredState {
        databases: dbs.iter().map(|n| LogicalName::parse(n).unwrap()).collect(),
        ..DesiredState::default()
    }
}

async fn admit(
    db: &TestDb,
    project: &str,
    d: &DesiredState,
    key: Option<&str>,
) -> Result<platform_db::Admitted, AdmitError> {
    let project = ProjectId::parse(project).unwrap();
    let plan = Plan::build(&project, d).unwrap();
    put_desired(
        &db.pool,
        &project,
        d,
        &plan,
        key,
        OperationId(Uuid::now_v7()),
    )
    .await
}

const LEASE: Duration = Duration::from_secs(30);

#[tokio::test]
async fn admission_records_intent_and_bounds_in_flight_operations() {
    let db = TestDb::new().await;
    let first = admit(&db, "shop", &desired(&["main"]), None).await.unwrap();
    assert_eq!(first.revision, 1);
    let view = log::get(&db.pool, first.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(view.status, "pending");
    assert_eq!(view.steps.len(), 2);
    assert_eq!(view.steps[0].provider_name, "p_shop_main_owner");
    for _ in 1..MAX_IN_FLIGHT_PER_PROJECT {
        admit(&db, "shop", &desired(&["main"]), None).await.unwrap();
    }
    let refused = admit(&db, "shop", &desired(&["main"]), None).await;
    assert!(
        matches!(refused, Err(AdmitError::TooManyOperations { .. })),
        "{refused:?}"
    );
    // Another project is unaffected.
    admit(&db, "blog", &desired(&["main"]), None).await.unwrap();
    db.cleanup().await;
}

#[tokio::test]
async fn idempotency_keys_replay_or_conflict() {
    let db = TestDb::new().await;
    let a = admit(&db, "shop", &desired(&["main"]), Some("k-1"))
        .await
        .unwrap();
    let again = admit(&db, "shop", &desired(&["main"]), Some("k-1"))
        .await
        .unwrap();
    assert!(again.replayed);
    assert_eq!(again.operation_id, a.operation_id);
    let other = admit(&db, "shop", &desired(&["other"]), Some("k-1")).await;
    assert!(matches!(other, Err(AdmitError::IdempotencyConflict)));
    // Concurrent repeats of one key create exactly one operation.
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let pool = db.pool.clone();
        tasks.spawn(async move {
            let project = ProjectId::parse("race").unwrap();
            let d = desired(&["main"]);
            let plan = Plan::build(&project, &d).unwrap();
            put_desired(
                &pool,
                &project,
                &d,
                &plan,
                Some("same"),
                OperationId(Uuid::now_v7()),
            )
            .await
            .unwrap()
            .operation_id
        });
    }
    let ids: std::collections::BTreeSet<_> = tasks.join_all().await.into_iter().collect();
    assert_eq!(ids.len(), 1);
    db.cleanup().await;
}

#[tokio::test]
async fn a_lease_fences_out_its_previous_holder() {
    let db = TestDb::new().await;
    admit(&db, "shop", &desired(&["main"]), None).await.unwrap();
    let (w1, w2) = (Uuid::now_v7(), Uuid::now_v7());
    let mut first = log::claim(&db.pool, w1, Duration::from_millis(100))
        .await
        .unwrap()
        .unwrap();
    assert!(
        log::claim(&db.pool, w2, LEASE).await.unwrap().is_none(),
        "lease held"
    );
    first.operation.steps[0]
        .begin_attempt(&RetryPolicy::default())
        .unwrap();
    assert!(
        log::save_steps(
            &db.pool,
            &first.lease,
            &[&first.operation.steps[0]],
            Duration::from_millis(100)
        )
        .await
        .unwrap()
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    // The lease expired: another worker takes over and resumes the recorded state.
    let second = log::claim(&db.pool, w2, LEASE).await.unwrap().unwrap();
    assert_eq!(second.lease.token, first.lease.token + 1);
    assert_eq!(second.operation.steps[0].state, StepState::Uncertain);
    assert_eq!(second.operation.steps[0].attempts, 1);
    // The old holder can no longer write anything.
    first.operation.steps[0]
        .created(ProviderResourceId::parse("stale").unwrap())
        .unwrap();
    assert!(
        !log::save_steps(&db.pool, &first.lease, &[&first.operation.steps[0]], LEASE)
            .await
            .unwrap()
    );
    assert!(
        !log::finish(&db.pool, &first.lease, OperationState::Succeeded, &[])
            .await
            .unwrap()
    );
    let view = log::get(&db.pool, second.operation.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        view.steps[0].resource_id, None,
        "the fenced write left no trace"
    );
    assert_eq!(view.status, "uncertain");
    db.cleanup().await;
}

#[tokio::test]
async fn one_project_reconciles_one_operation_at_a_time_in_order() {
    let db = TestDb::new().await;
    let a = admit(&db, "shop", &desired(&["main"]), None).await.unwrap();
    let b = admit(&db, "shop", &desired(&["main", "two"]), None)
        .await
        .unwrap();
    let c = admit(&db, "blog", &desired(&["main"]), None).await.unwrap();
    let w = Uuid::now_v7();
    let first = log::claim(&db.pool, w, LEASE).await.unwrap().unwrap();
    let second = log::claim(&db.pool, w, LEASE).await.unwrap().unwrap();
    assert_eq!(first.operation.id, a.operation_id);
    assert_eq!(second.operation.id, c.operation_id, "b waits behind a");
    assert!(log::claim(&db.pool, w, LEASE).await.unwrap().is_none());
    let mut done = first;
    for step in &mut done.operation.steps {
        step.begin_attempt(&RetryPolicy::default()).unwrap();
        step.created(ProviderResourceId::parse(&format!("id-{}", step.position)).unwrap())
            .unwrap();
    }
    let steps: Vec<_> = done.operation.steps.iter().collect();
    assert!(
        log::finish(&db.pool, &done.lease, OperationState::Succeeded, &steps)
            .await
            .unwrap()
    );
    let third = log::claim(&db.pool, w, LEASE).await.unwrap().unwrap();
    assert_eq!(third.operation.id, b.operation_id);
    let status = platform_db::project_status(&db.pool, &ProjectId::parse("shop").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.desired_revision, 2);
    assert_eq!(
        status.observed.len(),
        2,
        "observed resources recorded with the steps"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn the_event_log_is_append_only() {
    let db = TestDb::new().await;
    admit(&db, "shop", &desired(&["main"]), None).await.unwrap();
    for statement in [
        "UPDATE operation_events SET event = 'forged'",
        "DELETE FROM operation_events",
        "TRUNCATE operation_events",
    ] {
        let error = sqlx::query(statement).execute(&db.pool).await.unwrap_err();
        assert!(
            error.to_string().contains("append-only"),
            "{statement}: {error}"
        );
    }
    db.cleanup().await;
}

#[tokio::test]
async fn retry_resumes_a_partial_operation_but_never_stale_intent() {
    let db = TestDb::new().await;
    let a = admit(&db, "shop", &desired(&["main"]), None).await.unwrap();
    let not_partial = log::retry(&db.pool, a.operation_id).await;
    assert!(matches!(not_partial, Err(RetryError::NotPartial(_))));
    let w = Uuid::now_v7();
    let mut claimed = log::claim(&db.pool, w, LEASE).await.unwrap().unwrap();
    let policy = RetryPolicy {
        max_attempts: 1,
        ..RetryPolicy::default()
    };
    let step = &mut claimed.operation.steps[0];
    step.begin_attempt(&policy).unwrap();
    step.failed(ErrorClass::Timeout).unwrap();
    step.begin_attempt(&policy).unwrap(); // exhausted → gave up
    assert_eq!(step.state, StepState::GaveUp);
    let steps = [&claimed.operation.steps[0]];
    assert!(
        log::finish(&db.pool, &claimed.lease, OperationState::Partial, &steps)
            .await
            .unwrap()
    );
    let view = log::get(&db.pool, a.operation_id).await.unwrap().unwrap();
    assert_eq!(view.status, "partial");
    assert_eq!(view.steps[0].status, "gave_up");
    assert_eq!(view.steps[0].last_error, Some("timeout"));

    log::retry(&db.pool, a.operation_id).await.unwrap();
    let view = log::get(&db.pool, a.operation_id).await.unwrap().unwrap();
    assert_eq!(
        (view.status, view.round, view.steps[0].status),
        ("pending", 1, "pending")
    );

    // Make it partial again, then supersede it with newer intent: retry is refused.
    let mut claimed = log::claim(&db.pool, w, LEASE).await.unwrap().unwrap();
    claimed.operation.steps[0].begin_attempt(&policy).unwrap();
    claimed.operation.steps[0]
        .failed(ErrorClass::Rejected)
        .unwrap();
    let steps = [&claimed.operation.steps[0]];
    assert!(
        log::finish(&db.pool, &claimed.lease, OperationState::Partial, &steps)
            .await
            .unwrap()
    );
    admit(&db, "shop", &desired(&["main"]), None).await.unwrap();
    assert!(matches!(
        log::retry(&db.pool, a.operation_id).await,
        Err(RetryError::Superseded)
    ));
    assert!(matches!(
        log::retry(&db.pool, OperationId(Uuid::now_v7())).await,
        Err(RetryError::NotFound)
    ));
    db.cleanup().await;
}
