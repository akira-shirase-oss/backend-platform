//! SSRF: the only outbound call is mail, and its links come from the configured
//! `public_url` — never from `Host`, `Origin` or any request field. A password
//! reset link that follows a spoofed Host would send a live reset token to the
//! attacker's origin.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use common::spawn;
use serde_json::json;

#[tokio::test]
async fn reset_links_come_from_the_configured_url_not_the_request() {
    // The mail budget is per client, so a raised budget is needed — otherwise the
    // later headers (x-forwarded-host last) are answered 429 and never exercised.
    let s = common::spawn_with(|(settings, _)| {
        settings.mail.per_address =
            starter_http::budget::Rate::new(64, std::time::Duration::from_secs(120));
    })
    .await;
    let client = reqwest::Client::new();
    // One address per header: the reset budget is small per address, so a shared
    // address would see later headers answered 429 — never exercised at all.
    for (i, (name, value)) in [
        ("host", "evil.example"),
        ("origin", "https://evil.example"),
        ("referer", "https://evil.example/x"),
        ("x-forwarded-host", "evil.example"),
    ]
    .iter()
    .enumerate()
    {
        let email = format!("victim{i}@example.test");
        let _ = s.verified_session(&email).await;
        let r = client
            .post(s.url("/v1/password-reset"))
            .header(*name, *value)
            .json(&json!({ "email": email }))
            .send()
            .await
            .unwrap();
        // A success, not a 429: the header must reach the code that builds the mail.
        assert!(
            r.status().is_success(),
            "{name} request refused before it could exercise the link builder: {}",
            r.status()
        );
        let mail = s
            .sink
            .latest_to(&email)
            .await
            .expect("reset mail was sent");
        assert!(
            mail.data.contains("https://app.example.test/"),
            "mail steered by the {name} header: {}",
            mail.data.lines().next().unwrap_or("")
        );
        assert!(
            !mail.data.contains("evil.example"),
            "attacker origin leaked into the mail via {name}"
        );
    }
}

#[tokio::test]
async fn user_controlled_fields_can_not_make_the_server_fetch() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    // URL-shaped strings in every public body are validated as data, never fetched.
    for (path, body) in [
        (
            "/v1/signup",
            json!({ "invite": "x", "email": "a@http://evil", "password": "x".repeat(12) }),
        ),
        (
            "/v1/verify",
            json!({ "token": "http://169.254.169.254/latest" }),
        ),
        (
            "/v1/session",
            json!({ "email": "a@b.test", "password": "http://evil.example" }),
        ),
        (
            "/v1/password-reset",
            json!({ "email": "file:///etc/passwd" }),
        ),
    ] {
        let r = client.post(s.url(path)).json(&body).send().await.unwrap();
        assert!(
            r.status().is_client_error() || r.status().as_u16() == 429,
            "{path} accepted {body}"
        );
    }
}
