//! Body attacks: oversized (declared and streamed), slow-dribbled, and malformed.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use std::time::Duration;

use common::{TOKEN, authed, config, raw, raw_status, spawn, spawn_with};
use reqwest::Method;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn a_body_past_the_limit_is_413_whether_declared_or_streamed() {
    let s = spawn().await; // max_body_bytes = 16_384
    let client = reqwest::Client::new();

    // Declared: Content-Length over the limit.
    let big = vec![b'x'; 16_385];
    let r = client
        .put(s.url("/v1/projects/shop/desired"))
        .bearer_auth(TOKEN)
        .header("content-type", "application/json")
        .body(big)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 413);

    // Streamed: chunked encoding, limit crossed mid-body (server must cut it).
    let mut stream = TcpStream::connect(s.addr()).await.unwrap();
    let head = format!(
        "PUT /v1/projects/shop/desired HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    for _ in 0..6 {
        let chunk = format!("{:x}\r\n{}\r\n", 4_096, "x".repeat(4_096));
        if stream.write_all(chunk.as_bytes()).await.is_err() {
            break;
        }
        let mut buf = [0_u8; 512];
        let _ = tokio::time::timeout(Duration::from_millis(300), stream.read(&mut buf))
            .await
            .map(|r| r.map(|n| out.extend_from_slice(&buf[..n])));
        if String::from_utf8_lossy(&out).starts_with("HTTP/1.1 413") {
            break;
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut out)).await;
    let response = String::from_utf8_lossy(&out);
    assert_eq!(raw_status(&response), 413, "{response}");
}

#[tokio::test]
async fn a_slow_dribbled_body_times_out_without_taking_the_server_down() {
    let s = spawn_with({
        let mut c = config();
        c.request_timeout = Duration::from_millis(600);
        c
    })
    .await;
    let mut stream = TcpStream::connect(s.addr()).await.unwrap();
    let head = format!(
        "PUT /v1/projects/slow/desired HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: 8000\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    // Dribble a byte at a time: the request deadline must cut us off.
    let mut response = Vec::new();
    for _ in 0..4 {
        if stream.write_all(b"x").await.is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut buf = [0_u8; 512];
        match tokio::time::timeout(Duration::from_millis(50), stream.read(&mut buf)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => response.extend_from_slice(&buf[..n]),
            _ => {}
        }
    }
    // The server must answer and close: read_to_end finishing inside the bound is
    // the close — a timeout here means the connection was held open forever.
    let read =
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut response)).await;
    assert!(read.is_ok(), "server held the slow connection open");
    let text = String::from_utf8_lossy(&response);
    assert_eq!(
        raw_status(&text),
        503,
        "slow body expected a 503, got {text:?}"
    );
    assert!(
        text.contains("\"code\":\"timeout\""),
        "expected the timeout envelope, got {text:?}"
    );
    // The instance is still healthy — the attack consumed nothing it shouldn't.
    let client = reqwest::Client::new();
    let r = client.get(s.url("/livez")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
}

#[tokio::test]
async fn malformed_json_and_wrong_content_type_are_4xx_never_5xx() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    for (ctype, body) in [
        ("application/json", b"not json{".as_slice()),
        ("application/json", b"\xff\xfe\x00broken".as_slice()),
        ("text/plain", b"{}".as_slice()),
        ("application/xml", b"<x/>".as_slice()),
    ] {
        let r = client
            .put(s.url("/v1/projects/shop/desired"))
            .bearer_auth(TOKEN)
            .header("content-type", ctype)
            .body(body.to_vec())
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        assert!((400..500).contains(&status), "{ctype} {body:?} → {status}");
    }
    // Structurally valid JSON with an unknown field is refused, not silently dropped.
    let r = client
        .put(s.url("/v1/projects/shop/desired"))
        .bearer_auth(TOKEN)
        .json(&json!({ "databases": [], "evil": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
    // Method confusion is refused.
    let r = authed(&client, Method::POST, &s.url("/v1/projects/shop")).await;
    assert_eq!(r.status().as_u16(), 405);
}

/// A head carrying both Content-Length and Transfer-Encoding is ambiguous framing —
/// request smuggling. Hyper's rule (RFC 9112): Transfer-Encoding wins and
/// Content-Length is ignored — the disambiguation keeps two readers from
/// disagreeing about where the body ends. A body that only parses as chunked
/// proves it (CL: 4 would truncate the frame; a wrong credential masks it, 401
/// lands before the body is read — so the request carries the real TOKEN).
#[tokio::test]
async fn a_head_with_content_length_and_transfer_encoding_reads_chunked() {
    let s = spawn().await;
    let body = "{\"databases\":[\"main\"]}";
    // A second request is pipelined behind the first: RFC 9112 §6.1 requires the
    // server to close the connection after an ambiguous-framing response, so the
    // pipelined request must never be answered — exactly one response, then EOF.
    let raw_request = format!(
        "PUT /v1/projects/shop/desired HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n\
         GET /v1/operation-counts HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\n\r\n",
        body.len()
    );
    let reply = raw(&s, raw_request.as_bytes(), Duration::from_secs(5)).await;
    // The chunked body is what the handler saw: 202. Had Content-Length: 4 won,
    // the body would have been four bytes of frame bytes — invalid JSON, a 4xx.
    assert_eq!(
        raw_status(&reply),
        202,
        "the chunked body did not win over the declared Content-Length: {reply:?}"
    );
    assert_eq!(
        reply.matches("HTTP/1.1 ").count(),
        1,
        "the connection stayed open and answered the pipelined request: {reply:?}"
    );
}

/// Two Content-Length headers that disagree is the same ambiguity.
#[tokio::test]
async fn conflicting_content_length_headers_are_refused() {
    let s = spawn().await;
    let reply = raw(
        &s,
        b"POST /v1/projects/x/desired HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer t\r\nContent-Type: application/json\r\nContent-Length: 4\r\nContent-Length: 8000\r\n\r\n{}",
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(raw_status(&reply), 400, "conflicting CL refused: {reply:?}");
}
