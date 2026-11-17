//! Load shedding (F13): beyond the in-flight bound a request is refused at once with 503
//! `overloaded`, never queued for a database connection; and the bound stays at most four
//! times the pool (D-032), so admitted requests can actually get a connection.
#![allow(clippy::unwrap_used, reason = "test: a panic is the failure report")]

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt as _;
use platform_server::config::Secret;
use platform_server::{AppState, Config, app};
use tower::ServiceExt as _;

const TOKEN: &str = "test-admin-token-0123456789abcdef";

#[test]
fn the_in_flight_bound_is_at_most_four_times_the_pool() {
    let config = Config::default();
    assert!(
        config.max_in_flight <= 4 * config.db_max_connections as usize,
        "max_in_flight {} lets requests queue for {} connections",
        config.max_in_flight,
        config.db_max_connections
    );
}

#[tokio::test]
async fn a_full_server_sheds_at_once_with_503_overloaded() {
    let config = Config {
        admin_token: Secret::new(TOKEN.to_owned()),
        ..Config::default()
    };
    // No database is reachable: a request that got past the gate would wait for it.
    let settings = platform_db::PoolSettings {
        acquire_timeout: Duration::from_secs(5),
        ..platform_db::PoolSettings::default()
    };
    let dead =
        platform_db::connect_lazy("postgres://platform:x@127.0.0.1:1/platform", &settings).unwrap();
    let state = AppState::new(&config, dead);
    let held = state
        .gate
        .clone()
        .try_acquire_many_owned(u32::try_from(config.max_in_flight).unwrap())
        .unwrap();
    let router = app(&config, state);
    let started = Instant::now();
    let response = router
        .oneshot(
            Request::builder()
                .uri("/v1/operation-counts")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::RETRY_AFTER], "1");
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["error"]["code"], "overloaded");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "shed at once, not queued: {:?}",
        started.elapsed()
    );
    drop(held);
}
