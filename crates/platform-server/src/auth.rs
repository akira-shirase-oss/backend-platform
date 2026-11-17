//! B0 authentication: one bootstrap admin bearer token, compared in constant time.

use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::AppState;
use crate::error::ApiError;

/// Equal length and equal bytes, without an early exit on the first difference.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Refuses any request without `Authorization: Bearer <admin token>`.
pub async fn require_admin(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match presented {
        Some(token)
            if constant_time_eq(token.as_bytes(), state.admin_token.expose().as_bytes()) =>
        {
            next.run(request).await
        }
        _ => ApiError::Unauthorized.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn compares_exactly() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));
    }
}
