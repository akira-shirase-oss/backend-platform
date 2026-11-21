//! Authentication bypass: every shape of missing or malformed credentials is 401.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use std::time::Duration;

use common::{TOKEN, authed, raw, raw_status, spawn};
use reqwest::Method;

#[tokio::test]
async fn no_authorization_header_is_refused_on_every_protected_route() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    for path in [
        "/v1/projects/alpha",
        "/v1/projects/alpha/desired",
        "/v1/operations/0190c0f4-3b8c-7a00-8000-000000000000",
        "/v1/operation-counts",
    ] {
        for method in [Method::GET, Method::PUT, Method::POST] {
            let r = client
                .request(method.clone(), s.url(path))
                .send()
                .await
                .unwrap();
            // DECISIVE(no_authorization_header_is_refused_on_every_protected_route)
            assert_eq!(r.status().as_u16(), 401, "{method} {path}");
        }
    }
}

#[tokio::test]
async fn malformed_bearer_headers_are_refused() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let cases = [
        "Bearer attack-wrong-token-0000".to_owned(),
        format!("bearer {TOKEN}"),
        format!("BEARER {TOKEN}"),
        format!("Basic {TOKEN}"),
        TOKEN.to_owned(),
        format!("Bearer {TOKEN} extra"),
        format!("Bearer  {TOKEN}"),
        format!("Bearer\t{TOKEN}"),
    ];
    for case in &cases {
        let r = client
            .get(s.url("/v1/operation-counts"))
            .header("authorization", case)
            .send()
            .await
            .unwrap();
        // DECISIVE(malformed_bearer_headers_are_refused)
        assert_eq!(r.status().as_u16(), 401, "header {case:?}");
    }
    // Control: the real token reaches the handler.
    let r = authed(&client, Method::GET, &s.url("/v1/operation-counts")).await;
    assert_eq!(r.status().as_u16(), 200);
}

#[tokio::test]
async fn a_token_in_the_query_string_is_not_authentication() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let r = client
        .get(s.url(&format!("/v1/operation-counts?token={TOKEN}")))
        .send()
        .await
        .unwrap();
    // DECISIVE(a_token_in_the_query_string_is_not_authentication)
    assert_eq!(r.status().as_u16(), 401);
}

#[tokio::test]
async fn duplicated_authorization_headers_are_refused() {
    let s = spawn().await;
    // One wrong then one right, and one right then one wrong: either order must lose.
    for lines in [
        format!("Authorization: Bearer wrong\r\nAuthorization: Bearer {TOKEN}\r\n"),
        format!("Authorization: Bearer {TOKEN}\r\nAuthorization: Bearer wrong\r\n"),
    ] {
        let out = raw(
            &s,
            format!(
                "GET /v1/operation-counts HTTP/1.1\r\nHost: x\r\n{lines}Connection: close\r\n\r\n"
            )
            .as_bytes(),
            Duration::from_secs(3),
        )
        .await;
        // DECISIVE(duplicated_authorization_headers_are_refused)
        assert_eq!(raw_status(&out), 401, "{lines:?}");
    }
}

#[tokio::test]
async fn empty_and_control_byte_authorization_values_are_refused() {
    let s = spawn().await;
    for value in [
        "Authorization: Bearer\r\n",
        "Authorization: Bearer \x00tok\x01en\r\n",
    ] {
        let out = raw(
            &s,
            format!(
                "GET /v1/operation-counts HTTP/1.1\r\nHost: x\r\n{value}Connection: close\r\n\r\n"
            )
            .as_bytes(),
            Duration::from_secs(3),
        )
        .await;
        let status = raw_status(&out);
        // DECISIVE(empty_and_control_byte_authorization_values_are_refused)
        assert!(
            status == 400 || status == 401 || status == 431,
            "{value:?} → {out:?}"
        );
    }
}

#[tokio::test]
async fn health_and_readiness_stay_open_but_reveal_nothing() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let livez = client.get(s.url("/livez")).send().await.unwrap();
    assert_eq!(livez.status().as_u16(), 200);
    let body = livez.text().await.unwrap();
    // DECISIVE(health_and_readiness_stay_open_but_reveal_nothing)
    assert!(body.len() < 256, "livez leaks too much: {body:?}");
}
