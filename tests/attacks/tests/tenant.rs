//! Cross-tenant attempts: B0's one token is global, so the testable isolation is
//! project-scoped — another project's state must be invisible and its ids unguessable.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use common::{TOKEN, authed, spawn};
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn another_projects_state_is_not_visible() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let put = client
        .put(s.url("/v1/projects/alpha/desired"))
        .bearer_auth(TOKEN)
        .json(&json!({ "databases": ["main"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(put.status().as_u16(), 202);

    let alpha = authed(&client, Method::GET, &s.url("/v1/projects/alpha")).await;
    // DECISIVE(another_projects_state_is_not_visible)
    assert_eq!(alpha.status().as_u16(), 200);
    // "beta" must get nothing of alpha: 404 with the standard envelope, no leak.
    let beta = authed(&client, Method::GET, &s.url("/v1/projects/beta")).await;
    assert_eq!(beta.status().as_u16(), 404);
    let body = beta.text().await.unwrap();
    assert!(
        !body.contains("main"),
        "beta saw alpha's desired state: {body}"
    );
}

#[tokio::test]
async fn operation_ids_are_not_guessable() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    // A real operation must exist: a lookup widened to "any row" only leaks when
    // the table is not empty — an empty table keeps every guess at 404 either way.
    let put = client
        .put(s.url("/v1/projects/alpha/desired"))
        .bearer_auth(TOKEN)
        .json(&json!({ "databases": ["main"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(put.status().as_u16(), 202);
    for _ in 0..16 {
        let guess = uuid::Uuid::now_v7();
        let r = authed(
            &client,
            Method::GET,
            &s.url(&format!("/v1/operations/{guess}")),
        )
        .await;
        // DECISIVE(operation_ids_are_not_guessable)
        assert_eq!(r.status().as_u16(), 404, "guessed {guess}");
        let r = authed(
            &client,
            Method::POST,
            &s.url(&format!("/v1/operations/{guess}/retry")),
        )
        .await;
        assert!(r.status().is_client_error(), "guessed retry {guess}");
    }
}

#[tokio::test]
async fn path_traversal_and_encoding_tricks_are_refused() {
    let s = spawn().await;
    // raw(): a client's URL parser normalises `..` and decodes `%2e` before the
    // bytes leave, so the tricky path would never reach the server at all.
    let cases = [
        "/v1/projects/../projects/alpha",
        "/v1/projects/%2e%2e",
        "/v1/projects/a%2fb",
        "/v1/projects/N0T-VALID",
        "/v1/projects/%00",
        "/v1/projects/",
        "/v1/operations/not-a-uuid",
        "/v1/operations/../../../../etc/passwd",
    ];
    for path in cases {
        let response = common::raw(
            &s,
            format!("GET {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n").as_bytes(),
            std::time::Duration::from_secs(5),
        )
        .await;
        let status = common::raw_status(&response);
        // DECISIVE(path_traversal_and_encoding_tricks_are_refused)
        assert!(
            status == 400 || status == 404 || status == 405,
            "{path} → {status} (raw: {})",
            response.lines().next().unwrap_or("")
        );
    }
}
