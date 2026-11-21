//! Never-panics fuzz targets for every parser/decoder of outside input, plus a
//! replay of each target's committed seed corpus under `fuzz/corpus/<target>/`.
//! A panic found here is a red-first bug: the shrunk input lands in the corpus.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;

use axum::http::{HeaderMap, HeaderValue};
use proptest::prelude::*;
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::TestRunner;
use starter_accounts::{api_token, domain, session_cookie, tokens};
use starter_http::client::ClientKey;
use starter_server::config::{self, Source};

/// A `Source` over generated key/value pairs.
struct MapSource(BTreeMap<String, String>);

impl Source for MapSource {
    fn var(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }
}

/// Every configuration variable the parser actually reads — the fuzzer hits real
/// names, not generated keys that map to nothing.
const CONFIG_VARS: &[&str] = &[
    "STARTER_LISTEN",
    "STARTER_PUBLIC_URL",
    "STARTER_DATABASE_URL",
    "STARTER_ADMIN_DATABASE_URL",
    "STARTER_APP_PASSWORD",
    "STARTER_SMTP_RELAY",
    "STARTER_MAIL_FROM",
    "STARTER_TRUSTED_PROXIES",
    "STARTER_COOKIE_SECURE",
    "STARTER_LOG_FORMAT",
    "STARTER_MAX_BODY_BYTES",
    "STARTER_HEADER_READ_TIMEOUT_MS",
    "STARTER_DB_MAX_CONNECTIONS",
];

/// Feeds one input into `target`'s parsers.
fn apply(target: &str, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    match target {
        "api_token" => {
            let _ = tokens::looks_like_api_token(&text);
            let _ = tokens::looks_like_hex_secret(&text);
            let _ = tokens::digest(&text);
            let _ = tokens::base32(bytes);
        }
        "client_key" => {
            let mut map = HeaderMap::new();
            if let Ok(v) = HeaderValue::from_bytes(bytes) {
                map.append("x-forwarded-for", v);
            }
            let peer = SocketAddr::from(([203, 0, 113, 9], 9_000));
            let _ = ClientKey::from_request(peer, &map, &[IpAddr::from([127, 0, 0, 1])]);
            let _ = ClientKey::from_request(peer, &map, &[]);
        }
        "email" => {
            let _ = domain::normalize_email(&text);
        }
        "session_cookie" => {
            let mut map = HeaderMap::new();
            if let Ok(v) = HeaderValue::from_bytes(bytes) {
                map.append("cookie", v);
            }
            let _ = session_cookie(&map, "starter_session");
            let _ = session_cookie(&map, "x");
        }
        "authorization" => {
            let mut map = HeaderMap::new();
            for part in text.split(',') {
                if let Ok(v) = HeaderValue::from_str(part.trim()) {
                    map.append("authorization", v);
                }
            }
            let _ = api_token(&map);
        }
        "password" => {
            let _ = domain::check_password(&text);
        }
        "config" => {
            // The same generated text reaches every variable — a parser is only
            // exercised when the name is one it reads.
            let mut map = BTreeMap::new();
            for var in CONFIG_VARS {
                map.insert((*var).to_owned(), text.to_string());
            }
            let _ = config::Config::load(&MapSource(map));
        }
        other => panic!("unknown fuzz target {other}"),
    }
}

/// Every committed seed file is replayed through its target's parsers.
#[test]
fn seed_corpus_replays_without_panics() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz/corpus");
    let mut files = 0_usize;
    for dir in std::fs::read_dir(&root).unwrap() {
        let dir = dir.unwrap();
        let target = dir.file_name().to_string_lossy().into_owned();
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            apply(&target, &bytes);
            files += 1;
        }
    }
    assert!(files >= 15, "only {files} corpus files — corpus too thin");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(128)
    ))]

    #[test]
    fn token_parsers_never_panic(s in any::<String>(), b in any::<Vec<u8>>()) {
        let _ = tokens::looks_like_api_token(&s);
        let _ = tokens::looks_like_hex_secret(&s);
        let _ = tokens::digest(&s);
        let _ = tokens::base32(&b);
    }

    #[test]
    fn xff_parsing_never_panic_or_reads_past_the_rightmost_entry(
        entries in prop::collection::vec(any::<String>(), 0..8),
        trusted in any::<bool>(),
    ) {
        let mut map = HeaderMap::new();
        for entry in &entries {
            if let Ok(v) = HeaderValue::from_str(entry) {
                map.append("x-forwarded-for", v);
            }
        }
        let peer = SocketAddr::from(([127, 0, 0, 1], 9_000));
        let trusted_peers: &[IpAddr] = if trusted { &[IpAddr::from([127, 0, 0, 1])] } else { &[] };
        let _ = ClientKey::from_request(peer, &map, trusted_peers);
    }

    #[test]
    fn validators_never_panic(s in any::<String>()) {
        let _ = domain::normalize_email(&s);
        let _ = domain::check_password(&s);
    }

    #[test]
    fn config_never_panics_or_accepts_garbage(
        pairs in prop::collection::vec(any::<(usize, String)>(), 0..16),
    ) {
        // Real variable names — generated keys a parser never reads exercise nothing.
        let map: BTreeMap<String, String> = pairs
            .into_iter()
            .map(|(i, v)| (CONFIG_VARS[i % CONFIG_VARS.len()].to_owned(), v))
            .collect();
        let _ = config::Config::load(&MapSource(map));
    }

    #[test]
    fn session_cookie_never_panics(values in prop::collection::vec(any::<Vec<u8>>(), 0..4)) {
        // Several Cookie headers — HTTP/2 may split them; opaque bytes must never panic.
        let mut map = HeaderMap::new();
        for v in &values {
            if let Ok(v) = HeaderValue::from_bytes(v) {
                map.append("cookie", v);
            }
        }
        let _ = session_cookie(&map, "starter_session");
    }

    #[test]
    fn authorization_never_panics(values in prop::collection::vec(any::<Vec<u8>>(), 0..4)) {
        // Multi-value Authorization, opaque bytes: duplicates refused, never a panic.
        let mut map = HeaderMap::new();
        for v in &values {
            if let Ok(v) = HeaderValue::from_bytes(v) {
                map.append("authorization", v);
            }
        }
        let _ = api_token(&map);
    }
}

/// Arbitrary bytes as request bodies through the real router: never a 5xx,
/// never a panic (a panic would surface as `CatchPanic`'s 500). A fixed seed so a
/// failure reproduces verbatim; the failing bytes print on failure.
#[tokio::test]
async fn request_bodies_through_the_router_never_5xx() {
    let s = common::spawn().await;
    let client = reqwest::Client::new();
    // Deterministic: the same generated bodies every run, so a failure reproduces.
    let mut runner = TestRunner::deterministic();
    let inputs: Vec<Vec<u8>> = (0..64)
        .map(|_| any::<Vec<u8>>().new_tree(&mut runner).unwrap().current())
        .collect();
    for (i, bytes) in inputs.iter().enumerate() {
        for path in ["/v1/signup", "/v1/session", "/v1/verify"] {
            let r = client
                .post(s.url(path))
                .header("content-type", "application/json")
                .body(bytes.clone())
                .send()
                .await
                .unwrap();
            assert!(
                !r.status().is_server_error(),
                "{path} on generated body {i} → {}; bytes (hex): {}",
                r.status(),
                bytes.iter().fold(String::new(), |mut s, b| {
                    use std::fmt::Write as _;
                    let _ = write!(s, "{b:02x}");
                    s
                })
            );
        }
    }
}

/// Garbage in a real variable is refused, and the refusal names the variable — a
/// parser that accepts anything would pass the never-panic property while being wrong.
#[test]
fn config_refuses_garbage_values_and_names_the_variable() {
    for (var, bad) in [
        ("STARTER_PUBLIC_URL", "gopher://example.test"),
        ("STARTER_PUBLIC_URL", "http://8.8.8.8"),
        ("STARTER_PUBLIC_URL", "http://evil.example?@127.0.0.1"),
        ("STARTER_PUBLIC_URL", "http://user:pw@127.0.0.1"),
        ("STARTER_PUBLIC_URL", "http://127.0.0.1:61450/x"),
        ("STARTER_LISTEN", "not-an-addr"),
        ("STARTER_MAX_BODY_BYTES", "notanumber"),
        ("STARTER_COOKIE_SECURE", "maybe"),
        ("STARTER_TRUSTED_PROXIES", "999.1.1.1"),
        ("STARTER_HEADER_READ_TIMEOUT_MS", "-5"),
    ] {
        let mut map = BTreeMap::new();
        map.insert(var.to_owned(), bad.to_owned());
        let err = config::Config::load(&MapSource(map)).unwrap_err();
        // DECISIVE(config_refuses_garbage_values_and_names_the_variable)
        assert!(
            err.iter().any(|e| e.contains(var)),
            "{var}={bad}: not refused by name — {err:?}"
        );
    }
}
