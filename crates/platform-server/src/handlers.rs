//! Route handlers: validate, call the control store, answer. No provider is touched here.

use std::time::Duration;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use platform_domain::{DesiredState, OperationId, Plan, ProjectId};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;

/// Longest idempotency key.
pub const MAX_IDEMPOTENCY_KEY_LEN: usize = 128;

/// Liveness: the process answers.
pub async fn livez() -> &'static str {
    // ATTACK(livez-minimal)
    "ok"
}

/// Readiness: the control database answers within 500 ms; otherwise 503, so a load
/// balancer stops sending management traffic and the console shows "cannot manage".
pub async fn readyz(State(state): State<AppState>) -> Response {
    let probe = sqlx::query("SELECT 1").execute(&state.db);
    match tokio::time::timeout(Duration::from_millis(500), probe).await {
        Ok(Ok(_)) => Json(json!({ "status": "ready" })).into_response(),
        _ => ApiError::Unavailable(None).into_response(),
    }
}

fn project_id(raw: &str) -> Result<ProjectId, ApiError> {
    // ATTACK(project-id-parse)
    ProjectId::parse(raw).map_err(|e| ApiError::Invalid(e.to_string()))
}

fn operation_id(raw: &str) -> Result<OperationId, ApiError> {
    Uuid::parse_str(raw)
        .map(OperationId)
        .map_err(|_| ApiError::Invalid("operation id must be a UUID".to_owned()))
}

pub fn idempotency_key(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(value) = headers.get("idempotency-key") else {
        return Ok(None);
    };
    let key = value
        .to_str()
        .ok()
        // ATTACK(idempotency-key-shape)
        .filter(|k| {
            (1..=MAX_IDEMPOTENCY_KEY_LEN).contains(&k.len())
                && k.bytes().all(|b| b.is_ascii_graphic())
        })
        .ok_or_else(|| {
            ApiError::Invalid(format!(
                "Idempotency-Key must be 1 to {MAX_IDEMPOTENCY_KEY_LEN} printable characters"
            ))
        })?;
    Ok(Some(key.to_owned()))
}

/// `PUT /v1/projects/{project}/desired`: records the desired state as a new revision and
/// the operation that will reconcile it. `202 Accepted`: the worker does the provisioning.
pub async fn put_desired(
    State(state): State<AppState>,
    Path(project): Path<String>,
    headers: HeaderMap,
    body: Result<Json<DesiredState>, JsonRejection>,
) -> Result<Response, ApiError> {
    let project = project_id(&project)?;
    let Json(desired) = body.map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::PayloadTooLarge
        } else {
            ApiError::Invalid(e.body_text())
        }
    })?;
    let plan = Plan::build(&project, &desired).map_err(|e| ApiError::Invalid(e.to_string()))?;
    let key = idempotency_key(&headers)?;
    let admitted = platform_db::put_desired(
        &state.db,
        &project,
        &desired,
        &plan,
        key.as_deref(),
        OperationId(Uuid::now_v7()),
    )
    .await?;
    let location = format!("/v1/operations/{}", admitted.operation_id);
    let body = json!({
        "operation_id": admitted.operation_id,
        "revision": admitted.revision,
        "replayed": admitted.replayed,
        "steps": plan.resources.len(),
    });
    Ok((
        StatusCode::ACCEPTED,
        [(header::LOCATION, location)],
        Json(body),
    )
        .into_response())
}

/// `GET /v1/projects/{project}`: desired next to observed, and the latest operation.
pub async fn project_status(
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Response, ApiError> {
    let project = project_id(&project)?;
    let status = platform_db::project_status(&state.db, &project)
        .await?
        // ATTACK(operation-lookup)
        .ok_or(ApiError::NotFound)?;
    Ok(Json(status).into_response())
}

/// `GET /v1/operations/{id}`: the operation, its recorded steps and derived status.
pub async fn get_operation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = operation_id(&id)?;
    let operation = platform_db::log::get(&state.db, id)
        .await?
        // ATTACK(operation-lookup)
        .ok_or(ApiError::NotFound)?;
    Ok(Json(operation).into_response())
}

/// `POST /v1/operations/{id}/retry`: a new round of a partial operation.
pub async fn retry_operation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = operation_id(&id)?;
    platform_db::log::retry(&state.db, id).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "operation_id": id }))).into_response())
}

/// `GET /v1/operation-counts`: operations by state (the load run's convergence check).
pub async fn operation_counts(State(state): State<AppState>) -> Result<Response, ApiError> {
    Ok(Json(platform_db::log::counts(&state.db).await?).into_response())
}
