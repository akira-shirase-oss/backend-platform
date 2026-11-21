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

/// Holds a request open until aborted: authenticated, valid headers, body
/// dribbling forever. A dropped connection is re-established — the gate must
/// stay full under a loaded machine, not only inside one request's lifetime.
async fn hold(addr: String, name: &str) {
    loop {
        let Ok(mut stream) = TcpStream::connect(&addr).await else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        let head = format!(
            "PUT /v1/projects/hold-{name}/desired HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n"
        );
        if stream.write_all(head.as_bytes()).await.is_err() {
            continue;
        }
        let mut sink = [0_u8; 512];
        loop {
            if stream.write_all(b"1\r\nx\r\n").await.is_err() {
                break;
            }
            let _ = tokio::time::timeout(Duration::from_millis(80), stream.read(&mut sink)).await;
        }
    }
}

/// The shed's own evidence: 503 is also the timeout's status, so the envelope's
/// `overloaded` code is what proves a request was shed, not merely slow.
async fn shed_code(response: reqwest::Response) -> String {
    response
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|v| v["error"]["code"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[tokio::test]
async fn the_shed_gate_cannot_be_evaded_by_spoofed_client_headers() {
    let s = spawn_with({
        let mut c = config();
        c.max_in_flight = 2;
        // A normal-length deadline: held connections must not die of request_timeout
        // during the test, or the gate gets a free slot every cycle and a spoof can
        // slip through the gap (the reconnect loop still guards real drops).
        c.request_timeout = Duration::from_secs(30);
        c
    })
    .await;
    let client = reqwest::Client::new();

    // Fill both slots with held, dribbling requests, then wait on observed state:
    // poll until a request is shed — the "overloaded" envelope, not a timeout's —
    // so the spoof loop below only runs once the gate is provably full.
    let h1 = tokio::spawn(hold(s.addr().to_owned(), "a"));
    let h2 = tokio::spawn(hold(s.addr().to_owned(), "b"));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let r = authed(&client, Method::GET, &s.url("/v1/operation-counts")).await;
        if r.status().as_u16() == 503 && shed_code(r).await == "overloaded" {
            break;
        }
        // DECISIVE(the_shed_gate_cannot_be_evaded_by_spoofed_client_headers)
        assert!(
            std::time::Instant::now() < deadline,
            "the gate never filled"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Every spoof attempt must still be shed — 503 with the shed's envelope.
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
        assert_eq!(
            shed_code(r).await,
            "overloaded",
            "spoof {i} got a 503 that was not the shed's"
        );
    }

    // Dropping a JoinHandle only detaches — abort the held requests so their
    // connections close and the gate frees its slots now, then wait on observed
    // state instead of a fixed sleep: poll until a request is admitted again.
    h1.abort();
    h2.abort();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let r = authed(&client, Method::GET, &s.url("/v1/operation-counts")).await;
        if r.status().as_u16() == 200 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the gate did not recover: {}",
            r.status()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
