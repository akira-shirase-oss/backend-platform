//! The token scope guard itself: an API token may `Action::Read` its own workspace and
//! nothing else — every other action is `session_required` there, and anything at all
//! in another workspace is `not_found`. Route handlers also gate on `member()`, but the
//! guard in `Accounts::access` is the contract a copy's own routes will rely on.
#![allow(clippy::unwrap_used, reason = "test: a panic is the failure report")]

mod common;

use axum::http::{HeaderMap, header};
use common::Kit;
use starter_accounts::domain::Action;
use starter_accounts::error::AccountsError;

const PEER: &str = "203.0.113.9:4000";

#[tokio::test]
async fn a_token_reads_only_its_own_workspace_and_cannot_act() {
    let kit = Kit::new().await;
    kit.verified_user("owner@example.test").await;
    let session = kit.session(PEER, "owner@example.test").await;
    let own = kit.workspace(&session, "own").await;
    let other = kit.workspace(&session, "other").await;
    let (_id, token) = kit.token(&session, own).await;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );

    // Every non-Read action needs a session, even in the token's own workspace.
    for action in [
        Action::ListMembers,
        Action::ManageMembers,
        Action::ManageOwners,
        Action::ManageTokens,
        Action::Delete,
    ] {
        let err = kit
            .accounts
            .access(&headers, own, action)
            .await
            .unwrap_err();
        assert!(
            matches!(err, AccountsError::SessionRequired),
            "{action:?} in own workspace -> {err:?}"
        );
    }

    // Read in its own workspace is the one thing a token may do.
    let access = kit.accounts.access(&headers, own, Action::Read).await;
    assert!(access.is_ok(), "read in own workspace -> {access:?}");

    // Everything in another workspace is NotFound: the token cannot see it exists.
    for action in [
        Action::Read,
        Action::ListMembers,
        Action::ManageMembers,
        Action::ManageOwners,
        Action::ManageTokens,
        Action::Delete,
    ] {
        let err = kit
            .accounts
            .access(&headers, other, action)
            .await
            .unwrap_err();
        assert!(
            matches!(err, AccountsError::NotFound),
            "{action:?} in another workspace -> {err:?}"
        );
    }
}
