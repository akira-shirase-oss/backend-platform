//! Header spoofing: forwarding headers never elevate, and a spoofed request id is
//! either propagated verbatim (valid shape, by design) or replaced with a fresh UUID.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use common::spawn;
use uuid::Uuid;

#[tokio::test]
async fn forwarding_headers_do_not_elevate_an_anonymous_request() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let spoofs: [(&str, &str); 4] = [
        ("x-forwarded-for", "127.0.0.1"),
        ("x-real-ip", "127.0.0.1"),
        ("forwarded", "for=127.0.0.1;proto=https"),
        ("x-forwarded-host", "admin.internal"),
    ];
    for (name, value) in spoofs {
        let r = client
            .get(s.url("/v1/operation-counts"))
            .header(name, value)
            .send()
            .await
            .unwrap();
        // DECISIVE(forwarding_headers_do_not_elevate_an_anonymous_request)
        assert_eq!(r.status().as_u16(), 401, "{name}: {value}");
    }
}

#[tokio::test]
async fn a_valid_shaped_spoofed_request_id_is_propagated_and_an_invalid_one_replaced() {
    let s = spawn().await;
    let client = reqwest::Client::new();

    // Valid shape: propagated verbatim — documented correlation behaviour, asserted so
    // a future change that trusts it for more than logging is caught here.
    let spoofed = "attacker.42_corr";
    let r = client
        .get(s.url("/v1/operation-counts"))
        .bearer_auth(common::TOKEN)
        .header("x-request-id", spoofed)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok()),
        Some(spoofed)
    );

    // Invalid shape: the server's own UUIDv7 replaces it.
    for bad in ["has space", "quote\"injection", &"x".repeat(9_000)] {
        let r = client
            .get(s.url("/v1/operation-counts"))
            .bearer_auth(common::TOKEN)
            .header("x-request-id", bad)
            .send()
            .await
            .unwrap();
        let echoed = r
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        // DECISIVE(a_valid_shaped_spoofed_request_id_is_propagated_and_an_invalid_one_replaced)
        assert_ne!(echoed, bad, "invalid id {bad:?} was propagated");
        assert!(
            Uuid::parse_str(echoed).is_ok(),
            "replacement {echoed:?} is not a UUID"
        );
    }
}
