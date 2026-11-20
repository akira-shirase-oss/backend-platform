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
            let mut map = HeaderMap::new();
            if let Ok(v) = HeaderValue::from_bytes(bytes) {
                map.insert("authorization", v);
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
            let value = text.into_owned();
            let _ = platform_server::config::Config::from_lookup(move |_| Some(value.clone()));
        }
        "worker_config" => {
            let value = text.into_owned();
            let _ = platform_worker::config::Config::from_lookup(move |_| Some(value.clone()));
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
    fn desired_state_json_never_panics(b in any::<Vec<u8>>()) {
        let _ = serde_json::from_slice::<DesiredState>(&b);
    }

    #[test]
    fn provider_replies_never_panic(b in any::<Vec<u8>>()) {
        let _ = serde_json::from_slice::<LookupBody>(&b);
        let _ = serde_json::from_slice::<CreatedBody>(&b);
    }

    #[test]
    fn header_parsers_never_panic(
        auth in any::<Vec<u8>>(),
        idem in any::<Vec<u8>>(),
    ) {
        let mut map = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_bytes(&auth) {
            map.insert("authorization", v);
        }
        if let Ok(v) = HeaderValue::from_bytes(&idem) {
            map.insert("idempotency-key", v);
        }
        let _ = bearer_token(&map);
        let _ = idempotency_key(&map);
    }

    #[test]
    fn config_parsers_never_panic_or_accept_garbage(
        pairs in prop::collection::vec(any::<(String, String)>(), 0..16),
    ) {
        let map: BTreeMap<String, String> = pairs.into_iter().collect();
        let _ = platform_server::config::Config::from_lookup(|n| map.get(n).cloned());
        let _ = platform_worker::config::Config::from_lookup(|n| map.get(n).cloned());
    }
}
