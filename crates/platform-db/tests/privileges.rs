//! The runtime role cannot rewrite the operation log (F1): append-only holds against the
//! role the server and worker actually run as, not by convention.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

use platform_db::guard::{PrivilegeError, ensure_runtime_privileges};
use platform_db::put_desired;
use platform_db::testing::{Role, TestDb};
use platform_domain::{DesiredState, LogicalName, OperationId, Plan, ProjectId};
use uuid::Uuid;

async fn seed(db: &TestDb) {
    let project = ProjectId::parse("shop").unwrap();
    let desired = DesiredState {
        databases: vec![LogicalName::parse("main").unwrap()],
        ..DesiredState::default()
    };
    let plan = Plan::build(&project, &desired).unwrap();
    put_desired(
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

async fn events(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM operation_events")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Every known way around the append-only trigger, tried as the runtime role.
#[tokio::test]
async fn the_runtime_role_cannot_bypass_append_only() {
    let db = TestDb::new().await;
    seed(&db).await;
    let before = events(&db.pool).await;
    let bypasses: [&[&'static str]; 6] = [
        &["UPDATE operation_events SET event = 'forged'"],
        &["DELETE FROM operation_events"],
        &["TRUNCATE operation_events"],
        &[
            "SET LOCAL session_replication_role = replica",
            "UPDATE operation_events SET event = 'forged'",
        ],
        &[
            "ALTER TABLE operation_events DISABLE TRIGGER operation_events_append_only",
            "DELETE FROM operation_events",
        ],
        &[
            "DROP TRIGGER operation_events_no_truncate ON operation_events",
            "TRUNCATE operation_events",
        ],
    ];
    for statements in bypasses {
        let mut tx = db.pool.begin().await.unwrap();
        let mut refused = false;
        for statement in statements {
            if sqlx::query(*statement).execute(&mut *tx).await.is_err() {
                refused = true;
                break;
            }
        }
        let _ = tx.rollback().await;
        assert!(refused, "the runtime role got through: {statements:?}");
    }
    assert_eq!(events(&db.pool).await, before);
    db.cleanup().await;
}

/// Even an administrator in replica mode cannot silence the triggers (ENABLE ALWAYS).
#[tokio::test]
async fn replica_mode_does_not_silence_the_triggers() {
    let db = TestDb::new().await;
    seed(&db).await;
    let admin = db.connect_as(Role::Admin).await;
    let mut tx = admin.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *tx)
        .await
        .unwrap();
    let rewrite = sqlx::query("UPDATE operation_events SET event = 'forged'")
        .execute(&mut *tx)
        .await;
    assert!(rewrite.is_err(), "the trigger must fire in replica mode");
    let _ = tx.rollback().await;
    admin.close().await;
    db.cleanup().await;
}

/// The server and the worker refuse to start as a superuser or as the owner.
#[tokio::test]
async fn the_startup_guard_accepts_only_the_runtime_role() {
    let db = TestDb::new().await;
    let admin = db.connect_as(Role::Admin).await;
    let owner = db.connect_as(Role::Owner).await;
    assert!(matches!(
        ensure_runtime_privileges(&admin).await,
        Err(PrivilegeError::Superuser { .. })
    ));
    assert!(matches!(
        ensure_runtime_privileges(&owner).await,
        Err(PrivilegeError::Owner { .. })
    ));
    ensure_runtime_privileges(&db.pool).await.unwrap();
    admin.close().await;
    owner.close().await;
    db.cleanup().await;
}

/// Every function the schema defines pins its `search_path` (F16), so an object planted
/// in another schema can never be resolved inside a trigger.
#[tokio::test]
async fn schema_functions_pin_their_search_path() {
    let db = TestDb::new().await;
    let unpinned: Vec<String> = sqlx::query_scalar(
        "SELECT p.proname::text FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
          WHERE n.nspname = 'public'
            AND NOT coalesce('search_path=pg_catalog' = ANY (p.proconfig), false)",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert!(
        unpinned.is_empty(),
        "functions without a pinned search_path: {unpinned:?}"
    );
    db.cleanup().await;
}
