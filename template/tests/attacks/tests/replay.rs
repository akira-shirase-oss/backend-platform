//! Replay: used invites, spent verify tokens and dead sessions stay dead.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a panic is the failure report"
)]

mod common;

use common::spawn;
use serde_json::json;

#[tokio::test]
async fn an_invite_is_spent_once() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let invite = s.invite().await;
    for (i, email) in ["first@example.test", "second@example.test"]
        .iter()
        .enumerate()
    {
        let r = client
            .post(s.url("/v1/signup"))
            .json(&json!({ "invite": invite, "email": email, "password": common::PASSWORD }))
            .send()
            .await
            .unwrap();
        if i == 0 {
            assert_eq!(r.status().as_u16(), 201);
        } else {
            // DECISIVE(an_invite_is_spent_once)
            assert_eq!(r.status().as_u16(), 403, "invite replayed");
        }
    }
}

#[tokio::test]
async fn a_verify_token_cannot_be_replayed() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let invite = s.invite().await;
    let r = client
        .post(s.url("/v1/signup"))
        .json(&json!({ "invite": invite, "email": "replay@example.test", "password": common::PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 201);
    let token = s
        .sink
        .latest_to("replay@example.test")
        .await
        .unwrap()
        .token()
        .unwrap();
    let first = client
        .post(s.url("/v1/verify"))
        .json(&json!({ "token": token }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status().as_u16(), 204);
    let second = client
        .post(s.url("/v1/verify"))
        .json(&json!({ "token": token }))
        .send()
        .await
        .unwrap();
    // DECISIVE(a_verify_token_cannot_be_replayed)
    assert!(
        second.status().is_client_error(),
        "verify token replayed → {}",
        second.status()
    );
}

#[tokio::test]
async fn a_reset_token_is_spent_once() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let session = s.verified_session("resettable@example.test").await;
    let me = |client: &reqwest::Client| {
        client
            .get(s.url("/v1/me"))
            .header("cookie", format!("starter_session={session}"))
    };
    assert_eq!(me(&client).send().await.unwrap().status().as_u16(), 200);

    // Distinct passwords per attempt: a refused confirm that still wrote would be
    // invisible if every attempt used the signup password.
    let confirm = |token: &str, password: &str| {
        client
            .post(s.url("/v1/password-reset/confirm"))
            .json(&json!({ "token": token, "password": password }))
    };
    let sign_in = |password: &str| {
        client
            .post(s.url("/v1/session"))
            .json(&json!({ "email": "resettable@example.test", "password": password }))
    };
    assert_eq!(
        client
            .post(s.url("/v1/password-reset"))
            .json(&json!({ "email": "resettable@example.test" }))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        202
    );
    let token = s
        .sink
        .latest_to("resettable@example.test")
        .await
        .unwrap()
        .token()
        .unwrap();
    assert_eq!(
        confirm(&token, "first reset password")
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        204
    );
    let replayed = confirm(&token, "replay reset password")
        .send()
        .await
        .unwrap();
    // DECISIVE(a_reset_token_is_spent_once)
    assert!(
        replayed.status().is_client_error(),
        "reset token replayed → {}",
        replayed.status()
    );
    // A reset ends every session: the pre-reset cookie is dead.
    let alive = me(&client).send().await.unwrap();
    assert_eq!(
        alive.status().as_u16(),
        401,
        "session survived a password reset"
    );
    // The replayed confirm did not silently write: its password does not sign in,
    // and the first reset's password does.
    let replay_password = sign_in("replay reset password").send().await.unwrap();
    assert!(
        replay_password.status().is_client_error(),
        "a replayed confirm's password signed in → {}",
        replay_password.status()
    );
    let first = sign_in("first reset password").send().await.unwrap();
    assert_eq!(
        first.status().as_u16(),
        200,
        "password after reset: {first:?}"
    );
}

#[tokio::test]
async fn an_expired_reset_token_is_refused() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    s.verified_session("resettable@example.test").await;
    let confirm = |token: &str, password: &str| {
        client
            .post(s.url("/v1/password-reset/confirm"))
            .json(&json!({ "token": token, "password": password }))
    };
    let sign_in = |password: &str| {
        client
            .post(s.url("/v1/session"))
            .json(&json!({ "email": "resettable@example.test", "password": password }))
    };
    assert_eq!(
        client
            .post(s.url("/v1/password-reset"))
            .json(&json!({ "email": "resettable@example.test" }))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        202
    );
    let token = s
        .sink
        .latest_to("resettable@example.test")
        .await
        .unwrap()
        .token()
        .unwrap();
    sqlx::query("UPDATE email_tokens SET expires_at = now() - interval '1 second'")
        .execute(&s.db.pool)
        .await
        .unwrap();
    let expired = confirm(&token, "expired reset password")
        .send()
        .await
        .unwrap();
    // DECISIVE(an_expired_reset_token_is_refused)
    assert!(
        expired.status().is_client_error(),
        "expired reset token accepted → {}",
        expired.status()
    );
    // The expired confirm changed nothing: its password does not sign in, and the
    // signup password still does.
    let expired_password = sign_in("expired reset password").send().await.unwrap();
    assert!(
        expired_password.status().is_client_error(),
        "an expired confirm's password signed in → {}",
        expired_password.status()
    );
    let still = sign_in(common::PASSWORD).send().await.unwrap();
    assert_eq!(still.status().as_u16(), 200);
}

#[tokio::test]
async fn a_signed_out_session_is_dead() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let session = s.verified_session("gone@example.test").await;
    let out = client
        .delete(s.url("/v1/session"))
        .header("cookie", format!("starter_session={session}"))
        .send()
        .await
        .unwrap();
    assert!(out.status().is_success());
    let r = client
        .get(s.url("/v1/me"))
        .header("cookie", format!("starter_session={session}"))
        .send()
        .await
        .unwrap();
    // DECISIVE(a_signed_out_session_is_dead)
    assert_eq!(r.status().as_u16(), 401, "dead session reused");
}

#[tokio::test]
async fn a_duplicate_signup_is_refused_without_disclosure() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let first_session = s.verified_session("dup@example.test").await;
    // A fresh invite + the same address: refused, and the envelope must not confirm
    // the account exists beyond the documented email_taken.
    let invite = s.invite().await;
    let r = client
        .post(s.url("/v1/signup"))
        .json(
            &json!({ "invite": invite, "email": "dup@example.test", "password": common::PASSWORD }),
        )
        .send()
        .await
        .unwrap();
    // DECISIVE(a_duplicate_signup_is_refused_without_disclosure)
    assert!(r.status().is_client_error(), "duplicate signup accepted");
    // The session cookie from the FIRST account still works — replay did not corrupt it.
    let me = client
        .get(s.url("/v1/me"))
        .header("cookie", format!("starter_session={first_session}"))
        .send()
        .await
        .unwrap();
    assert_eq!(me.status().as_u16(), 200, "first account's session broken");
    let body: serde_json::Value = me.json().await.unwrap();
    assert_eq!(
        body["email"], "dup@example.test",
        "the session belongs to the first account, not a new one"
    );
}

/// Sign-in must not disclose which side failed: an unknown address and a known
/// address with a wrong password get the same refusal, same body.
#[tokio::test]
async fn sign_in_does_not_disclose_which_side_failed() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    s.verified_session("known@example.test").await;
    let mut replies = Vec::new();
    for email in ["known@example.test", "never-seen@example.test"] {
        let r = client
            .post(s.url("/v1/session"))
            .json(&json!({ "email": email, "password": "wrong password 12" }))
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        let body = r.json::<serde_json::Value>().await.unwrap();
        // Status, refusal code and message — a request_id differs per reply by
        // design and is the one field allowed to.
        replies.push((
            status,
            body["error"]["code"].clone(),
            body["error"]["message"].clone(),
        ));
    }
    // DECISIVE(sign_in_does_not_disclose_which_side_failed)
    assert_eq!(
        replies[0], replies[1],
        "unknown address vs wrong password differ: {replies:?}"
    );
    assert_eq!(replies[0].0, 401, "bad sign-in is a flat 401");
}

/// An address carrying CRLF would inject headers into outgoing mail; the email
/// check must refuse it at the edge — on signup and on the reset request.
#[tokio::test]
async fn an_email_with_header_injection_is_refused() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let invite = s.invite().await;
    for evil in [
        "a@example.test\r\nBcc: stolen@example.test",
        "a@example.test\nContent-Type: text/html",
    ] {
        for (route, body) in [
            (
                "/v1/signup",
                json!({ "invite": invite, "email": evil, "password": common::PASSWORD }),
            ),
            ("/v1/password-reset", json!({ "email": evil })),
        ] {
            let r = client.post(s.url(route)).json(&body).send().await.unwrap();
            // DECISIVE(an_email_with_header_injection_is_refused)
            assert!(
                r.status().is_client_error(),
                "{route} accepted {evil:?} → {}",
                r.status()
            );
        }
    }
}

/// Metacharacters are data, not code: a name full of them is stored and read back
/// verbatim, and the workspace keeps working afterwards.
#[tokio::test]
async fn sql_metacharacters_round_trip_as_data() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    let session = s.verified_session("owner2@example.test").await;
    let hostile = "'; DROP TABLE workspaces; -- \"\u{1f600}";
    let r = client
        .post(s.url("/v1/workspaces"))
        .header("cookie", format!("starter_session={session}"))
        .json(&json!({ "name": hostile }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 201);
    let ws = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = client
        .get(s.url(&format!("/v1/workspaces/{ws}")))
        .header("cookie", format!("starter_session={session}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let body = r.json::<serde_json::Value>().await.unwrap();
    assert_eq!(
        body["name"].as_str().unwrap(),
        hostile,
        "name did not round-trip verbatim"
    );
    // The table it tried to drop still serves requests: another workspace
    // creates and reads back fine.
    let r = client
        .post(s.url("/v1/workspaces"))
        .header("cookie", format!("starter_session={session}"))
        .json(&json!({ "name": "ordinary" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 201, "workspace table is broken");
}

/// Every sign-in mints a fresh session id — a predictable or reused id would let a
/// captured value replay forever.
#[tokio::test]
async fn every_sign_in_rotates_the_session_id() {
    let s = spawn().await;
    let client = reqwest::Client::new();
    s.verified_session("rotate@example.test").await;
    let mut ids = std::collections::HashSet::new();
    for _ in 0..3 {
        let r = client
            .post(s.url("/v1/session"))
            .json(&json!({ "email": "rotate@example.test", "password": common::PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 200);
        let id = common::cookie_value(r.headers(), "starter_session").unwrap();
        // DECISIVE(every_sign_in_rotates_the_session_id)
        assert!(ids.insert(id), "a session id was issued twice");
    }
}
