//! API errors: stable codes, safe messages, one JSON envelope
//! `{"error": {"code", "message", "request_id"}}`. Internal causes are logged once, here,
//! and never returned.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use platform_db::{AdmitError, DbError, RetryError};
use serde_json::json;

/// An error the API returns.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("missing or invalid credentials")]
    Unauthorized,
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Invalid(String),
    #[error("the request body is too large")]
    PayloadTooLarge,
    #[error("{0}")]
    TooManyOperations(String),
    #[error("idempotency key reused with a different desired state")]
    IdempotencyConflict,
    #[error("{message}")]
    Conflict { code: &'static str, message: String },
    #[error("the server is at capacity; retry shortly")]
    Overloaded,
    #[error("the control plane cannot manage right now")]
    Unavailable(#[source] Option<DbError>),
    #[error("internal error")]
    Internal(#[source] anyhow::Error),
}

impl ApiError {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            // ATTACK(invalid-is-400)
            Self::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            Self::PayloadTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large"),
            Self::TooManyOperations(_) => (StatusCode::TOO_MANY_REQUESTS, "too_many_operations"),
            Self::IdempotencyConflict => (StatusCode::CONFLICT, "idempotency_conflict"),
            Self::Conflict { code, .. } => (StatusCode::CONFLICT, code),
            Self::Overloaded => (StatusCode::SERVICE_UNAVAILABLE, "overloaded"),
            Self::Unavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "control_plane_unavailable"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

/// Renders the envelope (the request id is added by [`crate::middleware`]).
pub fn envelope(status: StatusCode, code: &str, message: &str) -> Response {
    let mut response = (
        status,
        Json(json!({ "error": { "code": code, "message": message } })),
    )
        .into_response();
    if matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
    ) {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    response
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        match &self {
            Self::Internal(error) => {
                tracing::error!(error = format!("{error:#}"), "internal error");
            }
            Self::Unavailable(Some(cause)) => {
                tracing::warn!(
                    cause = platform_db::chain(cause),
                    "control database unavailable"
                );
            }
            Self::Unavailable(None) => tracing::warn!("control database unavailable"),
            _ => {}
        }
        envelope(status, code, &self.to_string())
    }
}

impl From<DbError> for ApiError {
    fn from(error: DbError) -> Self {
        if error.is_unavailable() {
            Self::Unavailable(Some(error))
        } else {
            Self::Internal(error.into())
        }
    }
}

impl From<AdmitError> for ApiError {
    fn from(error: AdmitError) -> Self {
        match error {
            AdmitError::TooManyOperations { .. } => Self::TooManyOperations(error.to_string()),
            AdmitError::IdempotencyConflict => Self::IdempotencyConflict,
            AdmitError::Db(db) => db.into(),
        }
    }
}

impl From<RetryError> for ApiError {
    fn from(error: RetryError) -> Self {
        match error {
            RetryError::NotFound => Self::NotFound,
            RetryError::NotPartial(_) => Self::Conflict {
                code: "not_partial",
                message: error.to_string(),
            },
            RetryError::Superseded => Self::Conflict {
                code: "superseded",
                message: error.to_string(),
            },
            RetryError::TooManyOperations { .. } => Self::TooManyOperations(error.to_string()),
            RetryError::Db(db) => db.into(),
        }
    }
}
