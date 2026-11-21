//! The control API in process, against a real PostgreSQL.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt as _;
use platform_db::testing::TestDb;
use platform_server::config::Secret;
use platform_server::{AppState, Config, app};
use serde_json::{Value, json};
use tower::ServiceExt as _;

const TOKEN: &str = "test-admin-token-0123456789abcdef";

fn config() -> Config {
    Config {
        admin_token: Secret::new(TOKEN.to_owned()),
        ..Config::default()
    }
}

fn router(db: &TestDb) -> Router {
    let config = config();
    app(&config, AppState::new(&config, db.pool.clone()).unwrap())
}

async fn call(
    router: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    extra: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    let request = match body {
        Some(body) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, value)
}

fn starter() -> Value {
    json!({ "databases": ["main"], "buckets": ["files"], "identity": { "clients": ["web"] } })
}

#[tokio::test]
async fn desired_state_is_admitted_and_visible_with_its_operation() {
    let db = TestDb::new().await;
    let r = router(&db);
    let (status, headers, body) =
        call(&r, "PUT", "/v1/projects/shop/desired", Some(starter()), &[]).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["steps"], 5);
    let id = body["operation_id"].as_str().unwrap().to_owned();
    assert_eq!(headers[header::LOCATION], format!("/v1/operations/{id}"));

    let (status, _, op) = call(&r, "GET", &format!("/v1/operations/{id}"), None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(op["status"], "pending");
    let names: Vec<_> = op["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["provider_name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        [
            "p_shop_main_owner",
            "p_shop_main",
            "p-shop-files",
            "p-shop-realm",
            "p-shop-web"
        ]
    );

    let (status, _, project) = call(&r, "GET", "/v1/projects/shop", None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(project["desired_revision"], 1);
    assert_eq!(
        project["observed"],
        json!([]),
        "nothing observed until a worker reports it"
    );
    assert_eq!(project["latest_operation"]["id"], id.as_str());

    let (status, _, counts) = call(&r, "GET", "/v1/operation-counts", None, &[]).await;
    assert_eq!(
        (status, counts["pending"].clone()),
        (StatusCode::OK, json!(1))
    );
    db.cleanup().await;
}

#[tokio::test]
async fn bad_requests_are_refused_with_the_envelope() {
    let db = TestDb::new().await;
    let r = router(&db);
    for (uri, body) in [
        ("/v1/projects/Shop/desired", starter()),
        (
            "/v1/projects/shop/desired",
            json!({ "databases": ["main"], "cache": ["x"] }),
        ),
        ("/v1/projects/shop/desired", json!({})),
        (
            "/v1/projects/shop/desired",
            json!({ "buckets": ["a", "a"] }),
        ),
    ] {
        let (status, headers, body) = call(&r, "PUT", uri, Some(body), &[]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} {body}");
        assert_eq!(body["error"]["code"], "invalid_request");
        assert_eq!(
            body["error"]["request_id"].as_str(),
            headers["x-request-id"].to_str().ok()
        );
    }
    let (status, _, body) = call(&r, "GET", "/v1/operations/not-a-uuid", None, &[]).await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (StatusCode::BAD_REQUEST, json!("invalid_request"))
    );
    let (status, _, body) = call(
        &r,
        "GET",
        "/v1/operations/0190f3f0-0000-7000-8000-000000000000",
        None,
        &[],
    )
    .await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (StatusCode::NOT_FOUND, json!("not_found"))
    );
    let huge = json!({ "databases": ["main"], "pad": "x".repeat(20_000) });
    let (status, _, body) = call(&r, "PUT", "/v1/projects/shop/desired", Some(huge), &[]).await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (StatusCode::PAYLOAD_TOO_LARGE, json!("payload_too_large"))
    );
    let (status, _, _) = call(
        &r,
        "PUT",
        "/v1/projects/shop/desired",
        Some(starter()),
        &[("idempotency-key", &"k".repeat(129))],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    db.cleanup().await;
}

#[tokio::test]
async fn credentials_are_required() {
    let db = TestDb::new().await;
    let r = router(&db);
    for authorization in [
        None,
        Some("Bearer wrong-token-wrong-token-wrong-token"),
        Some(TOKEN),
    ] {
        let mut request = Request::builder().uri("/v1/operation-counts");
        if let Some(value) = authorization {
            request = request.header(header::AUTHORIZATION, value);
        }
        let response = r
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{authorization:?}"
        );
    }
    let response = r
        .clone()
        .oneshot(
            Request::builder()
                .uri("/livez")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "probes need no credentials"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn idempotency_and_the_in_flight_cap_are_enforced_at_admission() {
    let db = TestDb::new().await;
    let r = router(&db);
    let key = [("idempotency-key", "req-1")];
    let (_, _, first) = call(
        &r,
        "PUT",
        "/v1/projects/shop/desired",
        Some(starter()),
        &key,
    )
    .await;
    let (status, _, again) = call(
        &r,
        "PUT",
        "/v1/projects/shop/desired",
        Some(starter()),
        &key,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        (again["operation_id"].clone(), again["replayed"].clone()),
        (first["operation_id"].clone(), json!(true))
    );
    let (status, _, body) = call(
        &r,
        "PUT",
        "/v1/projects/shop/desired",
        Some(json!({"buckets": ["x"]})),
        &key,
    )
    .await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (StatusCode::CONFLICT, json!("idempotency_conflict"))
    );
    for _ in 1..platform_db::MAX_IN_FLIGHT_PER_PROJECT {
        let (status, _, _) =
            call(&r, "PUT", "/v1/projects/shop/desired", Some(starter()), &[]).await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }
    let (status, headers, body) =
        call(&r, "PUT", "/v1/projects/shop/desired", Some(starter()), &[]).await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (StatusCode::TOO_MANY_REQUESTS, json!("too_many_operations"))
    );
    assert_eq!(headers[header::RETRY_AFTER], "1");
    let id = first["operation_id"].as_str().unwrap();
    let (status, _, body) =
        call(&r, "POST", &format!("/v1/operations/{id}/retry"), None, &[]).await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (StatusCode::CONFLICT, json!("not_partial"))
    );
    db.cleanup().await;
}

/// Control plane down: management answers 503 `control_plane_unavailable`, quickly, and
/// readiness fails — never a fabricated success.
#[tokio::test]
async fn a_dead_control_database_is_reported_honestly() {
    let settings = platform_db::PoolSettings {
        acquire_timeout: Duration::from_millis(300),
        ..platform_db::PoolSettings::default()
    };
    let dead =
        platform_db::connect_lazy("postgres://platform:x@127.0.0.1:1/platform", &settings).unwrap();
    let config = config();
    let r = app(&config, AppState::new(&config, dead).unwrap());
    let started = std::time::Instant::now();
    let (status, _, body) = call(&r, "GET", "/v1/projects/shop", None, &[]).await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!("control_plane_unavailable")
        )
    );
    let (status, _, _) = call(&r, "PUT", "/v1/projects/shop/desired", Some(starter()), &[]).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, _, _) = call(&r, "GET", "/readyz", None, &[]).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "refusals are fast: {:?}",
        started.elapsed()
    );
}

/// An empty or too-short admin token is refused at `AppState::new` (the review's item 9):
/// a config that skipped `from_env` validation must not produce a state that
/// authenticates `Bearer ` — anything or nothing — as admin.
#[tokio::test]
async fn an_empty_or_short_admin_token_is_refused() {
    let settings = platform_db::PoolSettings::default();
    let dead =
        platform_db::connect_lazy("postgres://platform:x@127.0.0.1:1/platform", &settings).unwrap();
    for token in ["", "short", "0123456789012345678901234567890"] {
        let config = Config {
            admin_token: Secret::new(token.to_owned()),
            ..Config::default()
        };
        assert!(
            AppState::new(&config, dead.clone()).is_err(),
            "a {len}-byte token must be refused",
            len = token.len()
        );
    }
    let config = Config {
        admin_token: Secret::new(TOKEN.to_owned()),
        ..Config::default()
    };
    assert!(AppState::new(&config, dead).is_ok());
}

/// No secret reaches the server's logs (D-015, F11): the admin token, database passwords
/// and credential-shaped URLs, across success, refusal and a dead database.
#[tokio::test]
async fn no_secret_reaches_the_server_logs() {
    let (logs, _capture) = platform_db::logcheck::capture();
    let db = TestDb::new().await;
    let r = router(&db);
    let _ = call(
        &r,
        "PUT",
        "/v1/projects/shop/desired",
        Some(starter()),
        &[("idempotency-key", "k-1")],
    )
    .await;
    let _ = call(
        &r,
        "PUT",
        "/v1/projects/shop/desired",
        Some(json!({"cache": 1})),
        &[],
    )
    .await;
    let _ = call(&r, "GET", "/v1/projects/shop", None, &[]).await;
    let wrong = r
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/operation-counts")
                .header(
                    header::AUTHORIZATION,
                    "Bearer wrong-token-wrong-token-wrong-token",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    let settings = platform_db::PoolSettings {
        acquire_timeout: Duration::from_millis(300),
        ..platform_db::PoolSettings::default()
    };
    let dead = platform_db::connect_lazy(
        "postgres://platform:hunter2-secret@127.0.0.1:1/platform",
        &settings,
    )
    .unwrap();
    let config = config();
    let dead_router = app(&config, AppState::new(&config, dead).unwrap());
    let (status, _, _) = call(&dead_router, "GET", "/v1/projects/shop", None, &[]).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let text = logs.text();
    assert!(
        text.contains("control database unavailable"),
        "the capture saw the server's logs"
    );
    platform_db::logcheck::assert_no_leaks(&text, &[TOKEN, "hunter2-secret", "k-1"], "server logs");
    db.cleanup().await;
}
