//! Replay: an `Idempotency-Key` replays the recorded operation instead of
//! duplicating it; reusing the key for a different payload is refused.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use common::{TOKEN, spawn};
use serde_json::{Value, json};

async fn put(
    s: &common::Instance,
    client: &reqwest::Client,
    key: &str,
    body: Value,
) -> reqwest::Response {
    client
        .put(s.url("/v1/projects/shop/desired"))
        .bearer_auth(TOKEN)
        .header("idempotency-key", key)
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn the_same_key_replays_the_same_operation() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let desired = json!({ "databases": ["main"], "buckets": ["media"] });

    let first = put(&s, &client, "delivery-1", desired.clone()).await;
    assert_eq!(first.status().as_u16(), 202);
    let first: Value = first.json().await.unwrap();
    assert_eq!(first["replayed"], false);

    let second = put(&s, &client, "delivery-1", desired).await;
    assert_eq!(second.status().as_u16(), 202);
    let second: Value = second.json().await.unwrap();
    assert_eq!(second["replayed"], true, "replay was not flagged: {second}");
    assert_eq!(second["operation_id"], first["operation_id"]);
    assert_eq!(second["revision"], first["revision"]);
}

#[tokio::test]
async fn the_same_key_with_a_different_payload_is_refused() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let first = put(&s, &client, "delivery-2", json!({ "databases": ["a"] })).await;
    assert_eq!(first.status().as_u16(), 202);

    let second = put(&s, &client, "delivery-2", json!({ "databases": ["b"] })).await;
    assert!(
        second.status().is_client_error(),
        "key reuse with a different payload → {}",
        second.status()
    );
}

#[tokio::test]
async fn malformed_idempotency_keys_are_refused() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    for key in ["", "has space", &"k".repeat(200), "tab\there"] {
        let r = client
            .put(s.url("/v1/projects/shop/desired"))
            .bearer_auth(TOKEN)
            .header("idempotency-key", key)
            .json(&json!({ "databases": [] }))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_client_error(), "key {key:?} → {}", r.status());
    }
}
