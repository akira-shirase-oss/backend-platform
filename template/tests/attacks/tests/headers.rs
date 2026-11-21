//! Header spoofing: `X-Forwarded-For` is believed only from a trusted proxy, and
//! only its right-most entry — forged entries to its left never shift the
//! sign-in budget off the real client.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use common::{PASSWORD, raw, raw_status, spawn_serve, spawn_with};
use serde_json::json;
use starter_accounts::Settings;
use starter_http::budget::Rate;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Sends a bad sign-in attempt; returns the status.
async fn bad_signin(client: &reqwest::Client, base: &str, xff: Option<&str>) -> u16 {
    let mut r = client
        .post(format!("{base}/v1/session"))
        .json(&json!({ "email": "flooder@example.test", "password": "x".repeat(12) }));
    if let Some(hop) = xff {
        r = r.header("x-forwarded-for", hop);
    }
    r.send().await.unwrap().status().as_u16()
}

#[tokio::test]
async fn xff_from_an_untrusted_peer_does_not_evasion_the_signin_budget() {
    // trusted_proxies empty → the peer is the client; XFF is decoration.
    let s = spawn_with(|(settings, _)| {
        settings.trusted_proxies = vec![];
        settings.sign_in.per_address = Rate::new(6, Duration::from_secs(30));
    })
    .await;
    let client = reqwest::Client::new();
    let mut throttled = 0_usize;
    for i in 0..12 {
        // Each attempt spoofs a different "client": the budget must not move.
        let status = bad_signin(&client, &s.base, Some(&format!("198.51.100.{i}"))).await;
        if status == 429 {
            throttled += 1;
        }
    }
    assert!(throttled > 0, "rotating XFF evaded the per-address budget");
}

#[tokio::test]
async fn from_a_trusted_proxy_only_the_rightmost_xff_entry_is_believed() {
    // The real peer is 127.0.0.1 (the test is the proxy); declare it trusted.
    let s = spawn_with(|(settings, _): &mut (Settings, _)| {
        settings.trusted_proxies = vec![std::net::IpAddr::from([127, 0, 0, 1])];
        settings.sign_in.per_address = Rate::new(6, Duration::from_secs(30));
    })
    .await;
    let client = reqwest::Client::new();
    let mut throttled = 0_usize;
    for i in 0..12 {
        // Forged entries on the left, the same true client on the right: still bucketed.
        let status = bad_signin(&client, &s.base, Some(&format!("10.0.0.{i}, 203.0.113.7"))).await;
        if status == 429 {
            throttled += 1;
        }
    }
    assert!(
        throttled > 0,
        "left-side forged XFF entries evaded the budget"
    );

    // And when the right-most entry is a fresh address each time, the budget follows
    // the claimed client — proving only the last hop counts.
    let s2 = spawn_with(|(settings, _): &mut (Settings, _)| {
        settings.trusted_proxies = vec![std::net::IpAddr::from([127, 0, 0, 1])];
        settings.sign_in.per_address = Rate::new(6, Duration::from_secs(30));
    })
    .await;
    let mut first_seven_ok = true;
    for i in 0..7 {
        let status = bad_signin(&client, &s2.base, Some(&format!("203.0.113.{i}"))).await;
        if status == 429 {
            first_seven_ok = false;
        }
    }
    assert!(
        first_seven_ok,
        "right-most rotation did not move the budget — the wrong entry is believed"
    );
}

/// A raw head with both Content-Length and Transfer-Encoding is ambiguous framing —
/// request smuggling. The reader must pick exactly one interpretation: chunked wins.
#[tokio::test]
async fn a_head_with_content_length_and_transfer_encoding_reads_chunked() {
    let s = spawn_serve().await;
    // Hyper's rule (RFC 9112): Transfer-Encoding wins, Content-Length is ignored.
    // A body that only parses as chunked — a real signup that needs the whole
    // body — proves the CL did not truncate it (CL: 4 would read `0\r\n\r`).
    let invite = s.invite().await;
    let body = format!(
        "{{\"invite\":{invite:?},\"email\":\"smuggle@example.test\",\"password\":\"{PASSWORD}\"}}"
    );
    let head = format!(
        "POST /v1/signup HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
        body.len()
    );
    let reply = raw(&s, head.as_bytes(), Duration::from_secs(5)).await;
    assert_eq!(
        raw_status(&reply),
        201,
        "the chunked body did not win over the declared Content-Length: {reply:?}"
    );
}

/// Two Content-Length headers that disagree is the same ambiguity.
#[tokio::test]
async fn conflicting_content_length_headers_are_refused() {
    let s = spawn_serve().await;
    let reply = raw(
        &s,
        b"POST /v1/signup HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 4\r\nContent-Length: 8000\r\n\r\n{}",
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(
        raw_status(&reply),
        400,
        "conflicting Content-Length must be refused, got {reply:?}"
    );
}

/// The serve loop bounds the request head (hyper's `max_buf_size`: the effective
/// limit is the cap plus one adaptive read, under ~2×64 KiB): a head far past it
/// is refused with 431 or a bare close — never routed. 256 KiB sits between the
/// real bound and hyper's ~408 KiB default, so removing the cap would route the
/// request — the mutation proof is live. The write itself can die on EPIPE when
/// the server cuts the connection mid-send; that is the cap too.
#[tokio::test]
async fn oversized_headers_are_431() {
    let s = spawn_serve().await;
    let mut stream = tokio::net::TcpStream::connect(s.addr()).await.unwrap();
    let mut head = String::from("GET /v1/me HTTP/1.1\r\nHost: x\r\nX-Pad: ");
    head.push_str(&"x".repeat(256 * 1024));
    head.push_str("\r\n\r\n");
    let _ = stream.write_all(head.as_bytes()).await;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut out)).await;
    let reply = String::from_utf8_lossy(&out);
    assert!(
        reply.starts_with("HTTP/1.1 431") || reply.is_empty(),
        "oversized head: {reply:?}"
    );
    assert!(
        !reply.contains(" 401") && !reply.contains(" 200"),
        "an oversized head was routed: {reply:?}"
    );
}

/// A head that never completes must be cut by the header-read deadline: one
/// header line a second outlives no budget. Without the deadline the loop below
/// runs to its bound and the assertion fails.
#[tokio::test]
async fn dribbled_headers_are_closed_by_the_deadline() {
    let s = spawn_serve().await;
    let mut stream = tokio::net::TcpStream::connect(s.addr()).await.unwrap();
    stream
        .write_all(b"GET /v1/me HTTP/1.1\r\nHost: x\r\n")
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut closed = false;
    while std::time::Instant::now() < deadline {
        if stream.write_all(b"X-Slow: y\r\n").await.is_err() {
            closed = true;
            break;
        }
        let mut buf = [0_u8; 512];
        if let Ok(Ok(0) | Err(_)) =
            tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf)).await
        {
            closed = true;
            break;
        }
    }
    assert!(closed, "a head dribbled for 30 s was never cut");
}
