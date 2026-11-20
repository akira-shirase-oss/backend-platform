//! Rate-limit evasion: the `/v1` concurrency gate is a global bound — rotating
//! client headers cannot buy more in-flight slots, and the gate recovers cleanly.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use std::time::Duration;

use common::{TOKEN, authed, config, spawn_with};
use reqwest::Method;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Holds a request open: authenticated, valid headers, body dribbling forever.
async fn hold(addr: String, name: &str) {
    let Ok(mut stream) = TcpStream::connect(&addr).await else {
        return;
    };
    let head = format!(
        "PUT /v1/projects/hold-{name}/desired HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n"
    );
    if stream.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    let mut sink = [0_u8; 512];
    for _ in 0..20 {
        if stream.write_all(b"1\r\nx\r\n").await.is_err() {
            return;
        }
        let _ = tokio::time::timeout(Duration::from_millis(80), stream.read(&mut sink)).await;
    }
}

#[tokio::test]
async fn the_shed_gate_cannot_be_evaded_by_spoofed_client_headers() {
    let s = spawn_with({
        let mut c = config();
        c.max_in_flight = 2;
        c.request_timeout = Duration::from_millis(1_200);
        c
    })
    .await;
    let client = reqwest::Client::new();

    // Fill both slots with held, dribbling requests.
    let h1 = tokio::spawn(hold(s.addr().to_owned(), "a"));
    let h2 = tokio::spawn(hold(s.addr().to_owned(), "b"));
    tokio::time::sleep(Duration::from_millis(250)).await;

    // Every spoof attempt must still be shed with 503 while the gate is full.
    for i in 0..6 {
        let r = client
            .get(s.url("/v1/operation-counts"))
            .bearer_auth(TOKEN)
            .header("x-forwarded-for", format!("198.51.100.{i}"))
            .header("x-real-ip", format!("203.0.113.{i}"))
            .header("user-agent", format!("evader-{i}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 503, "spoof {i} was not shed");
    }

    // When the held requests die (server-side deadline), the gate recovers.
    drop(h1);
    drop(h2);
    tokio::time::sleep(Duration::from_millis(1_400)).await;
    let r = authed(&client, Method::GET, &s.url("/v1/operation-counts")).await;
    assert_eq!(r.status().as_u16(), 200, "the gate did not recover");
}
