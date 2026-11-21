//! SSRF: `platform-server` makes no outbound calls at all (it holds no provider
//! credentials and never links `platform-adapters`, D-013/T1), so there is no
//! fetchable URL carrier. These tests prove the request surface cannot smuggle one.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use common::{TOKEN, authed, spawn};
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn desired_state_refuses_every_smuggled_url_shape() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let cases = [
        // An unknown field that would be a fetchable URL were it ever honoured.
        json!({ "databases": ["db"], "callback_url": "http://169.254.169.254/latest/meta-data" }),
        json!({ "webhook": "https://attacker.example/hook" }),
        // A URL-shaped string where only a logical name is legal.
        json!({ "databases": ["http://169.254.169.254/x"] }),
        json!({ "buckets": ["//evil.example/share"] }),
        json!({ "identity": { "clients": ["https://169.254.169.254"] } }),
    ];
    for (i, body) in cases.iter().enumerate() {
        let r = client
            .put(s.url("/v1/projects/ssrf/desired"))
            .bearer_auth(TOKEN)
            .json(body)
            .send()
            .await
            .unwrap();
        // DECISIVE(desired_state_refuses_every_smuggled_url_shape)
        assert!(
            r.status().is_client_error(),
            "case {i} was accepted: {body} → {}",
            r.status()
        );
    }
}

#[tokio::test]
async fn a_url_shaped_project_or_operation_is_never_a_target() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    for path in [
        "/v1/projects/http:%2f%2f169.254.169.254",
        "/v1/operations/http:%2f%2fevil.example",
        "/v1/projects/file:%2f%2f%2fetc%2fpasswd",
    ] {
        let r = authed(&client, Method::GET, &s.url(path)).await;
        assert!(r.status().is_client_error(), "{path} → {}", r.status());
    }
}
