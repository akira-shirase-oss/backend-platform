//! Desired-state writes: validate, bound, record the intent and its operation in one
//! transaction (no provider is touched here), and the derived project status.

use platform_domain::{DesiredState, OperationId, Plan, ProjectId};
use serde::Serialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{AdmitError, DbError, is_unique_violation};
use crate::log::{OperationSummary, latest_for_project};

/// Most unfinished operations one project may have (D-017; a proposed lab value).
pub const MAX_IN_FLIGHT_PER_PROJECT: i64 = 4;

/// An admitted desired-state write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Admitted {
    /// The operation that reconciles it.
    pub operation_id: OperationId,
    /// The desired-state revision it created.
    pub revision: i64,
    /// True when an earlier write with the same idempotency key was returned instead.
    pub replayed: bool,
}

/// Records `desired` as the project's new revision and creates the operation that
/// reconciles it, unless the project is at its in-flight cap. With an idempotency key, a
/// repeat of the same write returns the original operation; a different body is refused.
pub async fn put_desired(
    pool: &PgPool,
    project: &ProjectId,
    desired: &DesiredState,
    plan: &Plan,
    idempotency_key: Option<&str>,
    new_id: OperationId,
) -> Result<Admitted, AdmitError> {
    let desired_json =
        serde_json::to_value(desired).map_err(|e| DbError::Corrupt(e.to_string()))?;
    // Two passes at most: the second runs only when a concurrent write with the same
    // idempotency key won the race, and then finds that write.
    for pass in 0..2 {
        let mut tx = pool.begin().await?;
        // ATTACK(idempotency-replay)
        if let Some(key) = idempotency_key
            && let Some((id, revision, same)) = sqlx::query_as::<_, (Uuid, i64, bool)>(
                "SELECT id, revision, desired = $3 FROM operations
                  WHERE project = $1 AND idempotency_key = $2",
            )
            .bind(project.as_str())
            .bind(key)
            .bind(&desired_json)
            .fetch_optional(&mut *tx)
            .await?
        {
            tx.rollback().await?;
            return if same {
                Ok(Admitted {
                    operation_id: OperationId(id),
                    revision,
                    replayed: true,
                })
            } else {
                // ATTACK(idempotency-conflict)
                Err(AdmitError::IdempotencyConflict)
            };
        }
        // The upsert row-locks the project's desired state until commit, so concurrent
        // writes to one project serialize here and the in-flight count below is exact.
        let revision: i64 = sqlx::query_scalar(
            "INSERT INTO desired_states (project, revision, spec) VALUES ($1, 1, $2)
             ON CONFLICT (project) DO UPDATE
                SET revision = desired_states.revision + 1, spec = EXCLUDED.spec,
                    updated_at = now()
             RETURNING revision",
        )
        .bind(project.as_str())
        .bind(&desired_json)
        .fetch_one(&mut *tx)
        .await?;
        let in_flight: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM operations
              WHERE project = $1 AND state IN ('pending', 'running')",
        )
        .bind(project.as_str())
        .fetch_one(&mut *tx)
        .await?;
        if in_flight >= MAX_IN_FLIGHT_PER_PROJECT {
            return Err(AdmitError::TooManyOperations {
                in_flight,
                limit: MAX_IN_FLIGHT_PER_PROJECT,
            });
        }
        let inserted = sqlx::query(
            "INSERT INTO operations (id, project, revision, desired, idempotency_key)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(new_id.0)
        .bind(project.as_str())
        .bind(revision)
        .bind(&desired_json)
        .bind(idempotency_key)
        .execute(&mut *tx)
        .await;
        match inserted {
            Ok(_) => {}
            Err(error) if pass == 0 && is_unique_violation(&error) => continue,
            Err(error) => return Err(error.into()),
        }
        insert_steps(&mut tx, new_id, plan).await?;
        tx.commit().await?;
        return Ok(Admitted {
            operation_id: new_id,
            revision,
            replayed: false,
        });
    }
    Err(AdmitError::Db(DbError::Corrupt(
        "idempotency key race did not resolve".to_owned(),
    )))
}

async fn insert_steps(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: OperationId,
    plan: &Plan,
) -> Result<(), AdmitError> {
    let mut positions = Vec::with_capacity(plan.resources.len());
    let mut kinds = Vec::with_capacity(plan.resources.len());
    let mut logical = Vec::with_capacity(plan.resources.len());
    let mut names = Vec::with_capacity(plan.resources.len());
    let mut specs = Vec::with_capacity(plan.resources.len());
    for (i, resource) in plan.resources.iter().enumerate() {
        positions.push(i16::try_from(i).map_err(|e| DbError::Corrupt(e.to_string()))?);
        kinds.push(resource.kind.as_str());
        logical.push(resource.logical.as_str());
        names.push(resource.name.as_str());
        specs.push(
            serde_json::to_string(&resource.spec).map_err(|e| DbError::Corrupt(e.to_string()))?,
        );
    }
    sqlx::query(
        "WITH steps AS (
             INSERT INTO operation_steps
                    (operation_id, position, kind, logical_name, provider_name, spec)
             SELECT $1, p, k, l, n, s::jsonb
               FROM UNNEST($2::int2[], $3::text[], $4::text[], $5::text[], $6::text[])
                    AS u(p, k, l, n, s)
         )
         INSERT INTO operation_events (operation_id, event) VALUES ($1, 'created')",
    )
    .bind(id.0)
    .bind(&positions)
    .bind(&kinds)
    .bind(&logical)
    .bind(&names)
    .bind(&specs)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// A resource the providers reported, by deterministic name.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ObservedRow {
    /// The kind.
    pub kind: String,
    /// The deterministic name.
    pub provider_name: String,
    /// The provider's id.
    pub resource_id: String,
    /// The operation that last observed it.
    pub operation_id: Uuid,
    /// When.
    #[serde(serialize_with = "time::serde::rfc3339::serialize")]
    pub observed_at: OffsetDateTime,
}

/// A project's desired state next to what was observed: the derived status (invariant 1).
#[derive(Debug, Clone, Serialize)]
pub struct ProjectStatus {
    /// The project.
    pub project: String,
    /// The latest desired revision.
    pub desired_revision: i64,
    /// The latest desired state.
    pub desired: serde_json::Value,
    /// What providers reported.
    pub observed: Vec<ObservedRow>,
    /// The latest operation, if any.
    pub latest_operation: Option<OperationSummary>,
}

/// The project's derived status, or `None` for a project that never wrote desired state.
pub async fn project_status(
    pool: &PgPool,
    project: &ProjectId,
) -> Result<Option<ProjectStatus>, DbError> {
    let Some((desired_revision, desired)) = sqlx::query_as::<_, (i64, serde_json::Value)>(
        // ATTACK(project-scope)
        "SELECT revision, spec FROM desired_states WHERE project = $1",
    )
    .bind(project.as_str())
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let observed = sqlx::query_as::<_, ObservedRow>(
        "SELECT kind, provider_name, resource_id, operation_id, observed_at
           FROM observed_resources WHERE project = $1 ORDER BY kind, provider_name",
    )
    .bind(project.as_str())
    .fetch_all(pool)
    .await?;
    let latest_operation = latest_for_project(pool, project).await?;
    Ok(Some(ProjectStatus {
        project: project.to_string(),
        desired_revision,
        desired,
        observed,
        latest_operation,
    }))
}
