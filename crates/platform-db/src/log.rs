//! The operation log the reconciler works from.
//!
//! A worker *claims* an operation under a lease; the claim increments the operation's
//! fencing token. Every later write names the token, so a worker whose lease was taken
//! over (it crashed, stalled, or was killed) can no longer change the record. Operations of
//! one project are claimed strictly in order, one at a time, so two workers never reconcile
//! the same deterministic names concurrently.
//!
//! Each write is one SQL statement (data-modifying CTEs): fence check and lease renewal,
//! step updates, the append-only events and observed resources commit together or not at
//! all, in a single round trip.

use std::time::Duration;

use platform_domain::{Operation, OperationId, OperationState, ProjectId, Step, TransitionError};
use serde::Serialize;
use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;
use crate::desired::MAX_IN_FLIGHT_PER_PROJECT;
use crate::rows::{STEP_COLUMNS, StepColumns, StepRow};

/// The right to write one operation's steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    /// The operation.
    pub operation: OperationId,
    /// The worker holding it.
    pub worker: Uuid,
    /// The fencing token of this claim.
    pub token: i64,
}

/// A claimed operation, as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    /// The lease.
    pub lease: Lease,
    /// The operation's recorded state.
    pub operation: Operation,
}

fn millis(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// Claims the oldest operation that is pending, or running under an expired lease (its
/// worker crashed), and whose project has no older unfinished operation.
pub async fn claim(
    pool: &PgPool,
    worker: Uuid,
    lease_for: Duration,
) -> Result<Option<Claimed>, DbError> {
    let row = sqlx::query_as::<_, (Uuid, String, i64, i32, i64)>(
        "WITH candidate AS (
             SELECT o.id FROM operations o
              WHERE o.state IN ('pending', 'running')
                AND (o.lease_expires_at IS NULL OR o.lease_expires_at < now())
                AND NOT EXISTS (
                    SELECT 1 FROM operations e
                     WHERE e.project = o.project AND e.state IN ('pending', 'running')
                       AND e.seq < o.seq)
              ORDER BY o.seq
              LIMIT 1
                FOR UPDATE SKIP LOCKED
         ), claimed AS (
             UPDATE operations o
                SET state = 'running', lease_owner = $1, lease_token = o.lease_token + 1,
                    lease_expires_at = now() + ($2::bigint * interval '1 millisecond'),
                    updated_at = now()
               FROM candidate WHERE o.id = candidate.id
          RETURNING o.id, o.project, o.revision, o.round, o.lease_token
         ), event AS (
             INSERT INTO operation_events (operation_id, event, lease_token)
             SELECT id, 'claimed', lease_token FROM claimed
         )
         SELECT id, project, revision, round, lease_token FROM claimed",
    )
    .bind(worker)
    .bind(millis(lease_for))
    .fetch_optional(pool)
    .await?;
    let Some((id, project, revision, round, token)) = row else {
        return Ok(None);
    };
    let steps = load_steps(pool, id).await?;
    let operation = Operation {
        id: OperationId(id),
        project: ProjectId::parse(&project).map_err(|e| DbError::Corrupt(e.to_string()))?,
        revision: u64::try_from(revision).map_err(|e| DbError::Corrupt(e.to_string()))?,
        round: u16::try_from(round).map_err(|e| DbError::Corrupt(e.to_string()))?,
        state: OperationState::Running,
        steps,
    };
    Ok(Some(Claimed {
        lease: Lease {
            operation: OperationId(id),
            worker,
            token,
        },
        operation,
    }))
}

async fn load_steps<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    id: Uuid,
) -> Result<Vec<Step>, DbError> {
    let rows = sqlx::query_as::<_, StepRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {STEP_COLUMNS} FROM operation_steps WHERE operation_id = $1 ORDER BY position"
    )))
    .bind(id)
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(StepRow::into_step).collect()
}

/// The one write statement: fence, renew or finish, steps, events, observed resources.
const WRITE: &str = "
WITH fence AS (
    UPDATE operations
       SET state = COALESCE($12::text, state),
           lease_owner = CASE WHEN $12::text IS NULL THEN lease_owner END,
           lease_expires_at = CASE WHEN $12::text IS NULL
                                   THEN now() + ($4::bigint * interval '1 millisecond') END,
           finished_at = CASE WHEN $12::text IS NULL THEN NULL ELSE now() END,
           updated_at = now()
     WHERE id = $1 AND lease_owner = $2 AND lease_token = $3 AND state = 'running'
 RETURNING id, project
), changed AS (
    UPDATE operation_steps AS s
       SET state = u.state, attempts = u.attempts, resource_id = u.resource_id,
           adopted = u.adopted, block_reason = u.block_reason, last_error = u.last_error,
           updated_at = now()
      FROM fence,
           UNNEST($5::int2[], $6::text[], $7::int2[], $8::text[], $9::bool[], $10::text[],
                  $11::text[])
           AS u(position, state, attempts, resource_id, adopted, block_reason, last_error)
     WHERE s.operation_id = fence.id AND s.position = u.position
 RETURNING s.position, s.state, s.attempts, s.last_error, s.kind, s.provider_name,
           s.resource_id
), events AS (
    INSERT INTO operation_events (operation_id, position, event, attempts, detail, lease_token)
    SELECT $1, c.position, c.state, c.attempts, c.last_error, $3 FROM changed c
    UNION ALL
    SELECT $1, NULL, $12::text, NULL, NULL, $3 FROM fence WHERE $12::text IS NOT NULL
), observed AS (
    INSERT INTO observed_resources (kind, provider_name, project, resource_id, operation_id)
    SELECT c.kind, c.provider_name, f.project, c.resource_id, $1
      FROM changed c, fence f WHERE c.state = 'done'
    ON CONFLICT (kind, provider_name) DO UPDATE
       SET resource_id = EXCLUDED.resource_id, operation_id = EXCLUDED.operation_id,
           observed_at = now()
)
SELECT count(*) FROM fence";

async fn write(
    pool: &PgPool,
    lease: &Lease,
    steps: &[&Step],
    lease_for: Duration,
    finish: Option<OperationState>,
) -> Result<bool, DbError> {
    let c = StepColumns::of(steps);
    let fenced_rows: i64 = sqlx::query_scalar(WRITE)
        .bind(lease.operation.0)
        .bind(lease.worker)
        .bind(lease.token)
        .bind(millis(lease_for))
        .bind(&c.positions)
        .bind(&c.states)
        .bind(&c.attempts)
        .bind(&c.resource_ids)
        .bind(&c.adopted)
        .bind(&c.block_reasons)
        .bind(&c.last_errors)
        .bind(finish.map(OperationState::as_str))
        .fetch_one(pool)
        .await?;
    Ok(fenced_rows == 1)
}

/// Saves steps and renews the lease. `Ok(false)`: the lease is gone (fenced) and nothing
/// was written; the caller must stop working on this operation.
pub async fn save_steps(
    pool: &PgPool,
    lease: &Lease,
    steps: &[&Step],
    lease_for: Duration,
) -> Result<bool, DbError> {
    write(pool, lease, steps, lease_for, None).await
}

/// Saves the last steps and ends the round as `succeeded` or `partial`, releasing the
/// lease. `Ok(false)`: fenced, nothing written.
pub async fn finish(
    pool: &PgPool,
    lease: &Lease,
    state: OperationState,
    steps: &[&Step],
) -> Result<bool, DbError> {
    debug_assert!(state.is_finished(), "finish takes a final state");
    write(pool, lease, steps, Duration::ZERO, Some(state)).await
}

/// One step, as shown by the API.
#[derive(Debug, Clone, Serialize)]
pub struct StepView {
    /// Position in the plan.
    pub position: u16,
    /// Kind.
    pub kind: &'static str,
    /// Name inside the project.
    pub name: String,
    /// Deterministic provider name.
    pub provider_name: String,
    /// Derived status: pending, uncertain, ready, blocked or `gave_up`.
    pub status: &'static str,
    /// Attempts this round.
    pub attempts: u16,
    /// The real provider id, once recorded.
    pub resource_id: Option<String>,
    /// True when an existing resource was adopted.
    pub adopted: Option<bool>,
    /// The last failure class.
    pub last_error: Option<&'static str>,
}

/// An operation, as shown by the API.
#[derive(Debug, Clone, Serialize)]
pub struct OperationSummary {
    /// Id.
    pub id: OperationId,
    /// Project.
    pub project: String,
    /// Desired revision it reconciles.
    pub revision: i64,
    /// Retry round.
    pub round: i32,
    /// Derived status: pending, running, uncertain, succeeded or partial.
    pub status: &'static str,
    /// Created.
    #[serde(serialize_with = "time::serde::rfc3339::serialize")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(serialize_with = "time::serde::rfc3339::serialize")]
    pub updated_at: OffsetDateTime,
    /// Its steps.
    pub steps: Vec<StepView>,
}

#[derive(Debug, sqlx::FromRow)]
struct OperationRow {
    id: Uuid,
    project: String,
    revision: i64,
    round: i32,
    state: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

async fn summary_of(pool: &PgPool, row: OperationRow) -> Result<OperationSummary, DbError> {
    let steps = load_steps(pool, row.id).await?;
    let state = OperationState::parse(&row.state)
        .ok_or_else(|| DbError::Corrupt(format!("operation state {}", row.state)))?;
    let operation = Operation {
        id: OperationId(row.id),
        project: ProjectId::parse(&row.project).map_err(|e| DbError::Corrupt(e.to_string()))?,
        revision: u64::try_from(row.revision).map_err(|e| DbError::Corrupt(e.to_string()))?,
        round: u16::try_from(row.round).map_err(|e| DbError::Corrupt(e.to_string()))?,
        state,
        steps,
    };
    Ok(OperationSummary {
        id: operation.id,
        status: operation.status(),
        project: row.project,
        revision: row.revision,
        round: row.round,
        created_at: row.created_at,
        updated_at: row.updated_at,
        steps: operation
            .steps
            .iter()
            .map(|s| StepView {
                position: s.position,
                kind: s.resource.kind.as_str(),
                name: s.resource.logical.to_string(),
                provider_name: s.resource.name.to_string(),
                status: s.state.status(),
                attempts: s.attempts,
                resource_id: s.resource_id().map(ToString::to_string),
                adopted: match s.state {
                    platform_domain::StepState::Done { adopted, .. } => Some(adopted),
                    _ => None,
                },
                last_error: s.last_error.map(platform_domain::ErrorClass::as_str),
            })
            .collect(),
    })
}

const OPERATION_COLUMNS: &str = "id, project, revision, round, state, created_at, updated_at";

/// One operation with its steps and derived status.
pub async fn get(pool: &PgPool, id: OperationId) -> Result<Option<OperationSummary>, DbError> {
    let row = sqlx::query_as::<_, OperationRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {OPERATION_COLUMNS} FROM operations WHERE id = $1"
    )))
    .bind(id.0)
    .fetch_optional(pool)
    .await?;
    match row {
        Some(row) => Ok(Some(summary_of(pool, row).await?)),
        None => Ok(None),
    }
}

/// The project's newest operation.
pub(crate) async fn latest_for_project(
    pool: &PgPool,
    project: &ProjectId,
) -> Result<Option<OperationSummary>, DbError> {
    let row = sqlx::query_as::<_, OperationRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {OPERATION_COLUMNS} FROM operations WHERE project = $1 ORDER BY seq DESC LIMIT 1"
    )))
    .bind(project.as_str())
    .fetch_optional(pool)
    .await?;
    match row {
        Some(row) => Ok(Some(summary_of(pool, row).await?)),
        None => Ok(None),
    }
}

/// Operation counts by state (for the load run's convergence check and the drills).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct OperationCounts {
    /// Waiting for a worker.
    pub pending: i64,
    /// Claimed.
    pub running: i64,
    /// Done.
    pub succeeded: i64,
    /// Visible partial outcomes.
    pub partial: i64,
}

/// Counts operations by state.
pub async fn counts(pool: &PgPool) -> Result<OperationCounts, DbError> {
    let rows =
        sqlx::query_as::<_, (String, i64)>("SELECT state, count(*) FROM operations GROUP BY state")
            .fetch_all(pool)
            .await?;
    let mut counts = OperationCounts::default();
    for (state, n) in rows {
        match state.as_str() {
            "pending" => counts.pending = n,
            "running" => counts.running = n,
            "succeeded" => counts.succeeded = n,
            "partial" => counts.partial = n,
            other => return Err(DbError::Corrupt(format!("operation state {other}"))),
        }
    }
    Ok(counts)
}

/// Why a retry was refused.
#[derive(Debug, Error)]
pub enum RetryError {
    #[error("no such operation")]
    NotFound,
    #[error("only a partial operation can be retried")]
    NotPartial(#[source] TransitionError),
    #[error("a newer operation exists for this project; retrying would reconcile stale intent")]
    Superseded,
    #[error("project has {in_flight} unfinished operations; the limit is {limit}")]
    TooManyOperations { in_flight: i64, limit: i64 },
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<sqlx::Error> for RetryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

/// Starts a new round of a partial operation (the domain decides what resets). Only the
/// project's newest operation may be retried: an older one would reconcile stale intent.
pub async fn retry(pool: &PgPool, id: OperationId) -> Result<OperationId, RetryError> {
    let mut tx = pool.begin().await?;
    let Some((project, seq)) =
        sqlx::query_as::<_, (String, i64)>("SELECT project, seq FROM operations WHERE id = $1")
            .bind(id.0)
            .fetch_optional(&mut *tx)
            .await?
    else {
        return Err(RetryError::NotFound);
    };
    // Lock order matches put_desired: the project's desired row, then the operation.
    sqlx::query("SELECT 1 FROM desired_states WHERE project = $1 FOR UPDATE")
        .bind(&project)
        .execute(&mut *tx)
        .await?;
    let (state, revision, round): (String, i64, i32) =
        sqlx::query_as("SELECT state, revision, round FROM operations WHERE id = $1 FOR UPDATE")
            .bind(id.0)
            .fetch_one(&mut *tx)
            .await?;
    let newer: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM operations WHERE project = $1 AND seq > $2)",
    )
    .bind(&project)
    .bind(seq)
    .fetch_one(&mut *tx)
    .await?;
    let mut operation = Operation {
        id,
        project: ProjectId::parse(&project).map_err(|e| DbError::Corrupt(e.to_string()))?,
        revision: u64::try_from(revision).map_err(|e| DbError::Corrupt(e.to_string()))?,
        round: u16::try_from(round).map_err(|e| DbError::Corrupt(e.to_string()))?,
        state: OperationState::parse(&state)
            .ok_or_else(|| DbError::Corrupt(format!("operation state {state}")))?,
        steps: load_steps(&mut *tx, id.0).await?,
    };
    operation.retry().map_err(RetryError::NotPartial)?;
    if newer {
        return Err(RetryError::Superseded);
    }
    let in_flight: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM operations WHERE project = $1 AND state IN ('pending', 'running')",
    )
    .bind(&project)
    .fetch_one(&mut *tx)
    .await?;
    if in_flight >= MAX_IN_FLIGHT_PER_PROJECT {
        return Err(RetryError::TooManyOperations {
            in_flight,
            limit: MAX_IN_FLIGHT_PER_PROJECT,
        });
    }
    let steps: Vec<&Step> = operation.steps.iter().collect();
    let c = StepColumns::of(&steps);
    sqlx::query(
        "WITH op AS (
             UPDATE operations
                SET state = 'pending', round = $2, lease_owner = NULL, lease_expires_at = NULL,
                    finished_at = NULL, updated_at = now()
              WHERE id = $1 AND state = 'partial'
          RETURNING id
         ), steps AS (
             UPDATE operation_steps AS s
                SET state = u.state, attempts = u.attempts, updated_at = now()
               FROM op, UNNEST($3::int2[], $4::text[], $5::int2[]) AS u(position, state, attempts)
              WHERE s.operation_id = op.id AND s.position = u.position
                AND (s.state, s.attempts) IS DISTINCT FROM (u.state, u.attempts)
         )
         INSERT INTO operation_events (operation_id, event, detail)
         SELECT id, 'retried', 'round ' || $2 FROM op",
    )
    .bind(id.0)
    .bind(i32::from(operation.round))
    .bind(&c.positions)
    .bind(&c.states)
    .bind(&c.attempts)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}
