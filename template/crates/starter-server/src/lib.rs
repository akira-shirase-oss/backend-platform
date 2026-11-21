//! The service skeleton a product copies: configuration ([`config`]), the health contract
//! ([`health`]) and the wiring ([`app`]). The product adds its own routes next to the
//! sample one, authorizing each with [`starter_accounts::Accounts::access`].

pub mod config;
pub mod health;

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use serde_json::json;
use starter_accounts::Accounts;
use starter_accounts::domain::Action;
use starter_accounts::error::AccountsError;
use starter_http::stack::{self, ErrorWindow, StackLimits};
use uuid::Uuid;

use crate::health::Health;

/// What the router needs.
#[derive(Debug, Clone)]
pub struct AppState {
    /// The accounts kit.
    pub accounts: Accounts,
    /// Readiness and the error window.
    pub health: Health,
    /// The error window the stack counts into.
    pub errors: Arc<ErrorWindow>,
    /// The stack's bounds.
    pub stack: StackLimits,
    /// Requests at once on `/v1` before shedding.
    pub max_in_flight: usize,
}

/// A sample product route: readable with a workspace's API token or by its members.
async fn hello(
    State(accounts): State<Accounts>,
    headers: axum::http::HeaderMap,
    Path(workspace): Path<Uuid>,
) -> Result<axum::Json<serde_json::Value>, AccountsError> {
    let access = accounts.access(&headers, workspace, Action::Read).await?;
    Ok(axum::Json(
        json!({ "workspace": access.workspace, "hello": "world" }),
    ))
}

/// A sample product write: any member may post a note; an API token may not —
/// `Action::Write` is the member-only boundary for a copy's own mutating routes.
async fn note(
    State(accounts): State<Accounts>,
    headers: axum::http::HeaderMap,
    Path(workspace): Path<Uuid>,
) -> Result<axum::Json<serde_json::Value>, AccountsError> {
    let access = accounts.access(&headers, workspace, Action::Write).await?;
    Ok(axum::Json(
        json!({ "workspace": access.workspace, "note": "saved" }),
    ))
}

/// `/v1`: the kit's routes merged with the product's, all inside the in-flight shed
/// gate. A copy merges its routes into `product` — `route_layer` here covers them,
/// which a `.layer` inside `router()` cannot (it wraps only the routes present when
/// it is added). The CSRF guard is not layered here: it wraps the whole app below.
pub fn v1(accounts: &Accounts, max_in_flight: usize) -> Router {
    let product = Router::new()
        .route("/workspaces/{workspace}/hello", get(hello))
        .route("/workspaces/{workspace}/note", post(note))
        .with_state(accounts.clone());
    starter_accounts::router(accounts.clone())
        .merge(product)
        .route_layer(axum::middleware::from_fn_with_state(
            stack::gate(max_in_flight),
            stack::shed,
        ))
}

/// The whole router with the middleware stack. `/livez`, `/readyz` and `/errorz` are
/// never shed; everything under `/v1` is.
pub fn app(state: AppState) -> Router {
    let health = Router::new()
        .route("/livez", get(health::livez))
        .route("/readyz", get(health::readyz))
        .route("/errorz", get(health::errorz))
        .with_state(state.health.clone());
    let router = Router::new()
        .nest("/v1", v1(&state.accounts, state.max_in_flight))
        .merge(health);
    let csrf =
        axum::middleware::from_fn_with_state(state.accounts.clone(), starter_accounts::csrf_guard);
    // The CSRF origin guard is a `.layer` over the complete app — the fallback
    // and any route a copy adds beside `v1` are covered too, where a `route_layer`
    // inside `v1` reaches only the routes present when it is added.
    // ATTACK(csrf-v1)
    let router = router.layer(csrf);
    stack::apply(router, state.stack, state.errors)
}
