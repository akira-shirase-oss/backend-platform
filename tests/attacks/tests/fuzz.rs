//! Never-panics fuzz targets for every parser/decoder of outside input, plus a
//! replay of each target's committed seed corpus under `fuzz/corpus/<target>/`.
//! A panic found here is a red-first bug: the shrunk input lands in the corpus.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use axum::http::{HeaderMap, HeaderValue};
use platform_adapters::wire::{CreatedBody, LookupBody};
use platform_domain::{DesiredState, LogicalName, OperationId, ProjectId};
use platform_server::auth::bearer_token;
use platform_server::handlers::idempotency_key;
use proptest::prelude::*;
use proptest::strategy::{Strategy, ValueTree};

/// The variables each config parser reads — generated keys would exercise nothing.
const SERVER_VARS: &[&str] = &[
    "BP_LISTEN",
    "BP_DATABASE_URL",
    "BP_ADMIN_TOKEN",
    "BP_MAX_BODY_BYTES",
    "BP_MAX_IN_FLIGHT",
    "BP_REQUEST_TIMEOUT_MS",
    "BP_SHUTDOWN_GRACE_MS",
    "BP_DB_MAX_CONNECTIONS",
    "BP_LOG_FORMAT",
];
const WORKER_VARS: &[&str] = &[
    "BP_DATABASE_URL",
    "BP_ADMIN_TOKEN",
    "BP_BACKOFF_BASE_MS",
    "BP_BACKOFF_CAP_MS",
    "BP_CALL_TIMEOUT_MS",
    "BP_DB_CALL_TIMEOUT_MS",
];

/// Feeds one corpus file (or one generated case) into `target`'s parsers.
fn apply(target: &str, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    match target {
        "project_id" => {
            let _ = ProjectId::try_from(text.to_string());
            let _ = serde_json::from_slice::<ProjectId>(bytes);
        }
        "operation_id" => {
            let _ = text.parse::<uuid::Uuid>().map(OperationId);
            let _ = serde_json::from_slice::<OperationId>(bytes);
        }
        "logical_name" => {
            let _ = LogicalName::try_from(text.to_string());
            let _ = serde_json::from_slice::<LogicalName>(bytes);
        }
        "provider_name" => {
            let _ = platform_domain::ProviderName::parse(&text);
        }
        "desired_state" => {
            let _ = serde_json::from_slice::<DesiredState>(bytes);
        }
        "authorization" => {
            // Split raw bytes on ',' so one corpus file can drive several header
            // values at once — including ones that are not UTF-8 (the whole point
            // of the to_str check: lossy text or from_str would destroy them).
            let mut map = HeaderMap::new();
            for part in bytes.split(|b| *b == b',') {
                if let Ok(v) = HeaderValue::from_bytes(part) {
                    map.append("authorization", v);
                }
            }
            let _ = bearer_token(&map);
        }
        "idempotency_key" => {
            let mut map = HeaderMap::new();
            if let Ok(v) = HeaderValue::from_bytes(bytes) {
                map.insert("idempotency-key", v);
            }
            let _ = idempotency_key(&map);
        }
        "lookup_body" => {
            let _ = serde_json::from_slice::<LookupBody>(bytes);
        }
        "created_body" => {
            let _ = serde_json::from_slice::<CreatedBody>(bytes);
        }
        "server_config" => {
            // Real variable names carry the value — a parser is only exercised on a
            // key it actually reads.
            let value = text.into_owned();
            let _ = platform_server::config::Config::from_lookup(move |n| {
                SERVER_VARS.contains(&n).then(|| value.clone())
            });
        }
        "worker_config" => {
            let value = text.into_owned();
            let _ = platform_worker::config::Config::from_lookup(move |n| {
                WORKER_VARS.contains(&n).then(|| value.clone())
            });
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
    assert!(files >= 20, "only {files} corpus files — corpus too thin");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(128)
    ))]

    #[test]
    fn ids_never_panic_on_arbitrary_text(s in any::<String>()) {
        let _ = ProjectId::try_from(s.clone());
        let _ = LogicalName::try_from(s.clone());
        let _ = platform_domain::ProviderName::parse(&s);
        let _ = s.parse::<uuid::Uuid>().map(OperationId);
        let _ = serde_json::from_str::<ProjectId>(&s);
    }

    #[test]
    fn desired_state_json_never_panics(
        b in any::<Vec<u8>>(),
        // Names that reach the length check: random bytes almost never form a
        // valid name field, so generate one — including over the limit.
        name in "[a-z_]{0,300}",
    ) {
        let _ = serde_json::from_slice::<DesiredState>(&b);
        let with_name = format!("{{\"databases\":[{name:?}]}}");
        let _ = serde_json::from_slice::<DesiredState>(with_name.as_bytes());
    }

    #[test]
    fn provider_replies_never_panic(
        b in any::<Vec<u8>>(),
        // Same: arbitrary bytes never reach the provider-id length check.
        id in "[ -~]{0,300}",
    ) {
        let _ = serde_json::from_slice::<LookupBody>(&b);
        let _ = serde_json::from_slice::<CreatedBody>(&b);
        let with_id = format!("{{\"id\":{id:?}}}");
        let _ = serde_json::from_slice::<CreatedBody>(with_id.as_bytes());
    }

    #[test]
    fn header_parsers_never_panic(
        auth in prop::collection::vec(any::<Vec<u8>>(), 0..4),
        idem in any::<Vec<u8>>(),
    ) {
        // Multi-value Authorization: duplicates must be refused, never panic.
        let mut map = HeaderMap::new();
        for v in &auth {
            if let Ok(v) = HeaderValue::from_bytes(v) {
                map.append("authorization", v);
            }
        }
        if let Ok(v) = HeaderValue::from_bytes(&idem) {
            map.insert("idempotency-key", v);
        }
        let _ = bearer_token(&map);
        let _ = idempotency_key(&map);
    }

    #[test]
    fn config_parsers_never_panic_or_accept_garbage(
        pairs in prop::collection::vec(any::<(usize, String)>(), 0..16),
    ) {
        // Real variable names — generated keys a parser never reads exercise nothing.
        let server: BTreeMap<String, String> = pairs
            .iter()
            .map(|(i, v)| (SERVER_VARS[i % SERVER_VARS.len()].to_owned(), v.clone()))
            .collect();
        let worker: BTreeMap<String, String> = pairs
            .iter()
            .map(|(i, v)| (WORKER_VARS[i % WORKER_VARS.len()].to_owned(), v.clone()))
            .collect();
        let _ = platform_server::config::Config::from_lookup(|n| server.get(n).cloned());
        let _ = platform_worker::config::Config::from_lookup(|n| worker.get(n).cloned());
    }
}

/// Arbitrary bytes as request bodies through the real router: never a 5xx, never a
/// panic. A deterministic runner so a failure reproduces; the failing bytes print.
#[tokio::test]
async fn request_bodies_through_the_router_never_5xx() {
    let s = common::spawn().await;
    let client = reqwest::Client::new();
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let inputs: Vec<Vec<u8>> = (0..64)
        .map(|_| any::<Vec<u8>>().new_tree(&mut runner).unwrap().current())
        .collect();
    for (i, bytes) in inputs.iter().enumerate() {
        for (method, path) in [
            (reqwest::Method::PUT, "/v1/projects/alpha/desired"),
            (reqwest::Method::POST, "/v1/projects/alpha/apply"),
        ] {
            let r = client
                .request(method.clone(), s.url(path))
                .bearer_auth(common::TOKEN)
                .header("content-type", "application/json")
                .body(bytes.clone())
                .send()
                .await
                .unwrap();
            assert!(
                !r.status().is_server_error(),
                "{method} {path} on generated body {i} → {}; bytes (hex): {}",
                r.status(),
                bytes.iter().fold(String::new(), |mut acc, b| {
                    use std::fmt::Write;
                    let _ = write!(acc, "{b:02x}");
                    acc
                })
            );
        }
    }
}

/// Garbage in a real variable is refused, and the refusal names the variable —
/// a parser that accepts anything passes never-panic while being wrong (other
/// missing variables would also error, so the message must name this one).
#[test]
fn config_refuses_garbage_values() {
    for bad in ["notanumber", "-5", &"9".repeat(20)] {
        let v = bad.to_string();
        let err = platform_server::config::Config::from_lookup(|n| {
            (n == "BP_MAX_BODY_BYTES").then(|| v.clone())
        })
        .unwrap_err();
        assert!(
            err.contains("BP_MAX_BODY_BYTES"),
            "BP_MAX_BODY_BYTES={bad} not refused by name: {err}"
        );
    }
    for bad in ["x", "  ", "a b"] {
        let v = bad.to_owned();
        let err = platform_server::config::Config::from_lookup(|n| {
            (n == "BP_ADMIN_TOKEN").then(|| v.clone())
        })
        .unwrap_err();
        assert!(
            err.contains("BP_ADMIN_TOKEN"),
            "BP_ADMIN_TOKEN={bad:?} not refused by name: {err}"
        );
    }
    let err = platform_worker::config::Config::from_lookup(|n| {
        (n == "BP_BACKOFF_BASE_MS").then(|| "z".to_owned())
    })
    .unwrap_err();
    assert!(
        err.contains("BP_BACKOFF_BASE_MS"),
        "BP_BACKOFF_BASE_MS=z not refused by name: {err}"
    );
}
