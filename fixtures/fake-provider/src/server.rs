//! The fake provider over HTTP: the `/v1` provider protocol of
//! [`platform_adapters::wire`], plus `/_fake` control routes for drills and load runs.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use platform_adapters::ProviderError;
use platform_adapters::wire::{CreateBody, CreatedBody, ResourceBody};
use platform_domain::{CreateRequest, Observation, ProviderName, ResourceKind};

use crate::{FakeProvider, FaultConfig, Reply};

/// Shared state: the fake and how long a hanging reply is held.
#[derive(Debug, Clone)]
pub struct AppState {
    /// The fake.
    pub fake: Arc<FakeProvider>,
    /// How long a hang lasts.
    pub hang: Duration,
}

/// The router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/livez", get(|| async { "ok" }))
        .route("/v1/{kind}/{name}", get(lookup))
        .route("/v1/{kind}", post(create))
        .route("/_fake/faults", put(set_faults))
        .route("/_fake/inventory", get(inventory))
        .route("/_fake/resources", get(resources))
        .route("/_fake/calls", get(calls))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(state)
}

fn error_status(error: ProviderError) -> StatusCode {
    match error {
        ProviderError::AlreadyExists => StatusCode::CONFLICT,
        ProviderError::Rejected => StatusCode::BAD_REQUEST,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn deliver<T>(hang: Duration, reply: Reply<T>, ok: impl FnOnce(T) -> Response) -> Response {
    if reply.hang {
        tokio::time::sleep(hang).await;
    }
    match reply.result {
        Ok(value) => ok(value),
        Err(error) => error_status(error).into_response(),
    }
}

async fn lookup(
    State(app): State<AppState>,
    Path((kind, name)): Path<(String, String)>,
) -> Response {
    let (Some(kind), Ok(name)) = (ResourceKind::parse(&kind), ProviderName::parse(&name)) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let reply = app.fake.lookup_now(kind, &name);
    deliver(app.hang, reply, |observation| match observation {
        Observation::Absent => StatusCode::NOT_FOUND.into_response(),
        Observation::Present(found) => Json(ResourceBody {
            id: found.id,
            spec: found.spec,
            tag: found.tag,
        })
        .into_response(),
    })
    .await
}

async fn create(
    State(app): State<AppState>,
    Path(kind): Path<String>,
    Json(body): Json<CreateBody>,
) -> Response {
    let Some(kind) = ResourceKind::parse(&kind) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if body.spec.kind() != kind {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let request = CreateRequest {
        kind,
        name: body.name,
        spec: body.spec,
        tag: body.tag,
        idempotency_key: body.idempotency_key,
    };
    let reply = app.fake.create_now(&request);
    deliver(app.hang, reply, |id| {
        (StatusCode::CREATED, Json(CreatedBody { id })).into_response()
    })
    .await
}

async fn set_faults(State(app): State<AppState>, Json(faults): Json<FaultConfig>) -> StatusCode {
    app.fake.set_faults(faults);
    StatusCode::NO_CONTENT
}

async fn inventory(State(app): State<AppState>) -> Response {
    Json(app.fake.inventory()).into_response()
}

async fn resources(State(app): State<AppState>) -> Response {
    Json(app.fake.resources()).into_response()
}

async fn calls(State(app): State<AppState>) -> Response {
    Json(app.fake.calls()).into_response()
}
