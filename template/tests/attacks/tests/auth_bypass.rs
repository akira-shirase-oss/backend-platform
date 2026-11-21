//! Authentication bypass: every route under `/v1` needs a real session or token.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use std::time::Duration;

use common::{body_json, raw, raw_status, spawn};
use serde_json::json;

#[tokio::test]
async fn protected_routes_refuse_anonymous_requests() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    for (method, path) in [
        ("GET", "/v1/me"),
        ("DELETE", "/v1/me"),
        ("POST", "/v1/workspaces"),
        ("GET", "/v1/workspaces/0190c0f4-3b8c-7a00-8000-000000000000"),
        (
            "GET",
            "/v1/workspaces/0190c0f4-3b8c-7a00-8000-000000000000/members",
        ),
        (
            "GET",
            "/v1/workspaces/0190c0f4-3b8c-7a00-8000-000000000000/tokens",
        ),
    ] {
        // Content-Type is set so extraction can't refuse first — the refusal asserted
        // here is the authentication refusal, 401. Handlers that validate their body
        // before the session check answer another 4xx — refused either way, never 2xx.
        let r = client
            .request(method.parse::<reqwest::Method>().unwrap(), s.url(path))
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        if method == "GET" {
            assert_eq!(status, 401, "{method} {path}");
        } else {
            assert!((400..500).contains(&status), "{method} {path} → {status}");
        }
    }
}

#[tokio::test]
async fn garbage_sessions_and_tokens_are_refused() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    for cookie in ["garbage", "st_deadbeef", "../../../etc/passwd", ""] {
        let r = client
            .get(s.url("/v1/me"))
            .header("cookie", format!("starter_session={cookie}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 401, "cookie {cookie:?}");
    }
    for token in ["st_", "st_garbage", "notatoken", "st_ almost right"] {
        let r = client
            .get(s.url("/v1/me"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 401, "token {token:?}");
    }
}

#[tokio::test]
async fn public_routes_stay_strict() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    // Signup without an invite is refused, never anonymous-created.
    let r = client
        .post(s.url("/v1/signup"))
        .json(&json!({ "invite": "wrong", "email": "a@b.test", "password": common::PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);
    // Verify with a made-up token is refused.
    let r = client
        .post(s.url("/v1/verify"))
        .json(&json!({ "token": "made-up-token" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());
    // Sign-in with wrong credentials is refused without disclosing which part.
    let r = client
        .post(s.url("/v1/session"))
        .json(&json!({ "email": "nobody@example.test", "password": "x".repeat(12) }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());
}

#[tokio::test]
async fn duplicate_authorization_headers_are_refused_in_either_order() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let alice = s.verified_session("dave@example.test").await;
    // A real API token: without the refusal, `[valid, junk]` would authenticate.
    let ws = client
        .post(s.url("/v1/workspaces"))
        .header("cookie", format!("starter_session={alice}"))
        .json(&json!({ "name": "team" }))
        .send()
        .await
        .unwrap();
    assert_eq!(ws.status().as_u16(), 201);
    let ws_id = body_json(ws).await["id"].as_str().unwrap().to_owned();
    let created = client
        .post(s.url(&format!("/v1/workspaces/{ws_id}/tokens")))
        .header("cookie", format!("starter_session={alice}"))
        .json(&json!({ "name": "ci" }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status().as_u16(), 201);
    let token = body_json(created).await["token"]
        .as_str()
        .unwrap()
        .to_owned();

    // Exactly one Authorization header is allowed; two values are ambiguous.
    for (label, lines) in [
        (
            "valid then junk",
            format!("Authorization: Bearer {token}\r\nAuthorization: Bearer junkjunk\r\n"),
        ),
        (
            "junk then valid",
            format!("Authorization: Bearer junkjunk\r\nAuthorization: Bearer {token}\r\n"),
        ),
        (
            "empty then valid",
            format!("Authorization:\r\nAuthorization: Bearer {token}\r\n"),
        ),
    ] {
        let res = raw(
            &s,
            format!("GET /v1/workspaces/{ws_id} HTTP/1.1\r\nHost: x\r\n{lines}\r\n").as_bytes(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(raw_status(&res), 401, "{label}: {res:?}");
    }
    // A malformed Authorization header must not fall back to a valid session cookie.
    let res = raw(
        &s,
        format!(
            "GET /v1/workspaces/{ws_id} HTTP/1.1\r\nHost: x\r\n\
             Authorization: Bearer junkjunk\r\nCookie: starter_session={alice}\r\n\r\n"
        )
        .as_bytes(),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(raw_status(&res), 401, "header+cookie: {res:?}");
}
