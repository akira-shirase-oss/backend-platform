//! The Tower stack. `ServiceBuilder` lists layers outermost first: a request travels down
//! the list, its response back up (rust-mastery chapter 12).

use std::any::Any;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, MatchedPath, Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::{self as axum_mw, Next};
use axum::response::{IntoResponse, Response};
use tokio::sync::Semaphore;
use tower::ServiceBuilder;
use tower_http::LatencyUnit;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{
    MakeRequestId, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::{DefaultOnFailure, DefaultOnResponse, TraceLayer};
use tracing::{Level, Span};

use crate::config::Config;
use crate::error::ApiError;

/// Request id header, both directions.
pub const REQUEST_ID_HEADER: &str = "x-request-id";
/// Longest client request id kept.
pub const MAX_REQUEST_ID_LEN: usize = 64;
/// Largest error body the envelope layer re-reads to add the request id.
const MAX_ERROR_BODY: usize = 8 * 1024;

/// UUIDv7 request ids.
#[derive(Debug, Clone, Copy, Default)]
pub struct MakeRequestUuidV7;

impl MakeRequestId for MakeRequestUuidV7 {
    fn make_request_id<B>(&mut self, _request: &Request<B>) -> Option<RequestId> {
        HeaderValue::from_str(&uuid::Uuid::now_v7().to_string())
            .ok()
            .map(RequestId::new)
    }
}

/// True for client ids safe to copy into logs: 1–64 of `[A-Za-z0-9._-]`.
pub fn is_acceptable_request_id(value: &str) -> bool {
    (1..=MAX_REQUEST_ID_LEN).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

async fn sanitize_request_id(mut request: Request) -> Request {
    let acceptable = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .map(|value| value.to_str().is_ok_and(is_acceptable_request_id));
    if acceptable == Some(false) {
        request.headers_mut().remove(REQUEST_ID_HEADER);
    }
    request
}

/// Root span: method, matched route (low cardinality), request id. Never the query string,
/// never a header value.
fn make_span(request: &Request<Body>) -> Span {
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("<unmatched>", MatchedPath::as_str);
    tracing::info_span!("request", method = %request.method(), route, request_id = %request_id)
}

fn code_for(status: StatusCode) -> &'static str {
    match status {
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::METHOD_NOT_ALLOWED => "method_not_allowed",
        StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
        StatusCode::SERVICE_UNAVAILABLE => "timeout",
        s if s.is_client_error() => "invalid_request",
        _ => "internal",
    }
}

/// Every 4xx/5xx leaves as the envelope, carrying the request id.
async fn error_envelope(request: Request, next: Next) -> Response {
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let response = next.run(request).await;
    let status = response.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return response;
    }
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes().starts_with(b"application/json"));
    let (mut parts, body) = response.into_parts();
    let mut value = if is_json {
        axum::body::to_bytes(body, MAX_ERROR_BODY)
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    } else {
        None
    }
    .unwrap_or_else(|| {
        let message = status.canonical_reason().unwrap_or("error");
        serde_json::json!({ "error": { "code": code_for(status), "message": message } })
    });
    if let (Some(id), Some(error)) = (
        request_id,
        value.get_mut("error").and_then(|e| e.as_object_mut()),
    ) {
        error.insert("request_id".to_owned(), serde_json::Value::String(id));
    }
    let bytes = serde_json::to_vec(&value).unwrap_or_default();
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(bytes))
}

/// A caught panic becomes a 500 envelope; the payload is logged, never returned.
fn panic_response(payload: Box<dyn Any + Send + 'static>) -> Response {
    let message = match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => payload.downcast::<&'static str>().map_or_else(
            |_| "non-string panic payload".to_owned(),
            |m| (*m).to_owned(),
        ),
    };
    ApiError::Internal(anyhow::anyhow!("handler panicked: {message}")).into_response()
}

/// Rejects at once when `max_in_flight` `/v1` requests are running, instead of queueing
/// until every request times out (T10).
pub async fn shed_load(
    State(gate): State<Arc<Semaphore>>,
    request: Request,
    next: Next,
) -> Response {
    let Ok(permit) = gate.try_acquire() else {
        return ApiError::Overloaded.into_response();
    };
    let response = next.run(request).await;
    drop(permit);
    response
}

/// Applies the global stack.
pub fn apply(router: Router, config: &Config) -> Router {
    let stack = ServiceBuilder::new()
        // 0. Credentials and idempotency keys are redacted from any Debug output.
        .layer(SetSensitiveRequestHeadersLayer::new([
            header::AUTHORIZATION,
            HeaderName::from_static("idempotency-key"),
        ]))
        // 1. Drop unsafe client ids before anything logs them.
        .layer(axum_mw::map_request(sanitize_request_id))
        // 2. Assign an id when absent. 3. Copy it onto every response.
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuidV7))
        .layer(PropagateRequestIdLayer::x_request_id())
        // 4. Root span, which sees the id.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(make_span)
                .on_response(
                    DefaultOnResponse::new()
                        .level(Level::DEBUG)
                        .latency_unit(LatencyUnit::Micros),
                )
                // Each error is logged once, where it is handled (crate::error); an ERROR
                // line per shed request would turn overload into a log storm.
                .on_failure(DefaultOnFailure::new().level(Level::DEBUG)),
        )
        // 5. One error envelope for everything below.
        .layer(axum_mw::from_fn(error_envelope))
        // 6. Panics become 500 envelopes.
        .layer(CatchPanicLayer::custom(panic_response))
        // 7. Deadline for the response: 503, we were slow, not the client.
        .layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            config.request_timeout,
        ))
        // 8–9. Body limits, by Content-Length up front and while streaming.
        .layer(RequestBodyLimitLayer::new(config.max_body_bytes))
        .layer(DefaultBodyLimit::max(config.max_body_bytes));
    router.layer(stack)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_id_policy() {
        for (input, expected) in [
            ("018f2f0e-3c1a-7cc0-9e1e-3f1f6b7f9a01", true),
            ("trace.42_a", true),
            ("", false),
            ("has space", false),
            ("new\nline", false),
            (&*"a".repeat(65), false),
        ] {
            assert_eq!(is_acceptable_request_id(input), expected, "{input:?}");
        }
    }

    #[test]
    fn panic_payloads_become_500s() {
        for payload in [
            Box::new("static") as Box<dyn Any + Send>,
            Box::new(String::from("owned")),
            Box::new(7_u8),
        ] {
            assert_eq!(
                panic_response(payload).status(),
                StatusCode::INTERNAL_SERVER_ERROR
            );
        }
    }
}
