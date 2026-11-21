//! Cross-tenant access: user B's session must see nothing of user A's workspace.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use common::{body_json, spawn};
use serde_json::{Value, json};

#[tokio::test]
async fn a_workspace_is_invisible_to_a_non_member() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let alice = s.verified_session("alice@example.test").await;
    let bob = s.verified_session("bob@example.test").await;

    // Alice creates a workspace.
    let created = client
        .post(s.url("/v1/workspaces"))
        .header("cookie", format!("starter_session={alice}"))
        .json(&json!({ "name": "Alice's shop" }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status().as_u16(), 201);
    let workspace = body_json(created).await["id"].as_str().unwrap().to_owned();

    // Every read/write of it under Bob's session is refused. Bodies are valid so the
    // refusal asserted is the access check, not JSON validation — and access failures
    // are exactly 404 + not_found so nothing confirms the workspace exists.
    for (method, path, body) in [
        ("GET", format!("/v1/workspaces/{workspace}"), None),
        ("DELETE", format!("/v1/workspaces/{workspace}"), None),
        ("GET", format!("/v1/workspaces/{workspace}/members"), None),
        (
            "POST",
            format!("/v1/workspaces/{workspace}/members"),
            Some(json!({ "email": "mallory@example.test", "role": "member" })),
        ),
        ("GET", format!("/v1/workspaces/{workspace}/tokens"), None),
        (
            "POST",
            format!("/v1/workspaces/{workspace}/tokens"),
            Some(json!({ "name": "ci" })),
        ),
    ] {
        let mut req = client
            .request(method.parse::<reqwest::Method>().unwrap(), s.url(&path))
            .header("cookie", format!("starter_session={bob}"));
        if let Some(body) = body {
            req = req.json(&body);
        }
        let r = req.send().await.unwrap();
        assert_eq!(r.status().as_u16(), 404, "{method} {path}");
        assert_eq!(
            body_json(r).await["error"]["code"],
            "not_found",
            "{method} {path}"
        );
    }

    // Re-read as the owner: the attempted writes and deletes changed nothing.
    let r = client
        .get(s.url(&format!("/v1/workspaces/{workspace}")))
        .header("cookie", format!("starter_session={alice}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    assert_eq!(body_json(r).await["name"].as_str().unwrap(), "Alice's shop");
    for path in ["members", "tokens"] {
        let r = client
            .get(s.url(&format!("/v1/workspaces/{workspace}/{path}")))
            .header("cookie", format!("starter_session={alice}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 200);
        let list = body_json(r).await;
        let entries = list.as_array().cloned().unwrap_or_else(|| {
            list.get("members")
                .or_else(|| list.get("tokens"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        });
        assert_eq!(
            entries.len(),
            usize::from(path == "members"),
            "{path}: {list}"
        );
    }
}

#[tokio::test]
async fn an_api_token_sees_only_its_own_workspace_and_only_reads() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let alice = s.verified_session("alice@example.test").await;

    // Alice owns two workspaces; a token minted on the first must not reach the second.
    let workspace = |name: &'static str| {
        let client = client.clone();
        let url = s.url("/v1/workspaces");
        let cookie = format!("starter_session={alice}");
        async move {
            let r = client
                .post(url)
                .header("cookie", cookie)
                .json(&json!({ "name": name }))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status().as_u16(), 201);
            body_json(r).await["id"].as_str().unwrap().to_owned()
        }
    };
    let first = workspace("first").await;
    let second = workspace("second").await;

    let minted = client
        .post(s.url(&format!("/v1/workspaces/{first}/tokens")))
        .header("cookie", format!("starter_session={alice}"))
        .json(&json!({ "name": "ci" }))
        .send()
        .await
        .unwrap();
    assert_eq!(minted.status().as_u16(), 201);
    let token = body_json(minted).await["token"]
        .as_str()
        .unwrap()
        .to_owned();

    // The token reads the workspace it is scoped to; member and token lists need a
    // session (403 session_required), and anything outside its own workspace is 404.
    for (path, want) in [
        (format!("/v1/workspaces/{first}"), 200),
        (format!("/v1/workspaces/{first}/members"), 403),
        (format!("/v1/workspaces/{first}/tokens"), 403),
        (format!("/v1/workspaces/{second}"), 404),
        (format!("/v1/workspaces/{second}/members"), 404),
    ] {
        let r = client
            .get(s.url(&path))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), want, "token GET {path}");
    }

    // Writes are refused even in the token's own workspace: 403 session_required.
    for (method, path, body) in [
        (
            "POST",
            format!("/v1/workspaces/{first}/tokens"),
            Some(json!({ "name": "x" })),
        ),
        (
            "POST",
            format!("/v1/workspaces/{first}/members"),
            Some(json!({ "email": "m@example.test", "role": "member" })),
        ),
        ("DELETE", format!("/v1/workspaces/{first}"), None),
    ] {
        let mut req = client
            .request(method.parse::<reqwest::Method>().unwrap(), s.url(&path))
            .bearer_auth(&token);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let r = req.send().await.unwrap();
        assert_eq!(r.status().as_u16(), 403, "token {method} {path}");
        assert_eq!(
            body_json(r).await["error"]["code"],
            "session_required",
            "token {method} {path}"
        );
    }
    // A token is never a session for account routes.
    let r = client
        .get(s.url("/v1/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);
}

#[tokio::test]
async fn resource_ids_of_other_users_are_not_guessable() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let alice = s.verified_session("carol@example.test").await;
    for _ in 0..8 {
        let guess = uuid::Uuid::now_v7();
        let r = client
            .get(s.url(&format!("/v1/workspaces/{guess}")))
            .header("cookie", format!("starter_session={alice}"))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_client_error(), "guessed {guess}");
    }
}

#[tokio::test]
async fn me_only_ever_returns_the_callers_own_account() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let dave = s.verified_session("dave@example.test").await;
    let r = client
        .get(s.url("/v1/me"))
        .header("cookie", format!("starter_session={dave}"))
        .header("x-user-id", uuid::Uuid::now_v7().to_string()) // spoof attempt
        .send()
        .await
        .unwrap();
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["email"].as_str().unwrap(), "dave@example.test");
}
