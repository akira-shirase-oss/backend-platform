//! The Backend Platform's control API.
//!
//! It writes desired state and the operations that reconcile it, and reads operations and
//! derived project status. It never calls a provider and holds no provider credential: the
//! worker does that work from the operation log (D-013, T1). When the control database is
//! unreachable it answers `503 control_plane_unavailable` — never a fabricated success.
//!
//! B0 authenticates with one bootstrap admin token; per-project principals, the audit log
//! and re-authentication for destructive operations are B1 (T5).

pub mod auth;
pub mod config;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod shutdown;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post, put};
use sqlx::PgPool;
use tokio::sync::Semaphore;

pub use config::Config;

/// Shared state.
#[derive(Debug, Clone)]
pub struct AppState {
    /// The control database.
    pub db: PgPool,
    /// The bootstrap admin token.
    pub admin_token: Arc<config::Secret>,
    /// `/v1` concurrency gate.
    pub gate: Arc<Semaphore>,
}

impl AppState {
    /// Builds the state, refusing an empty or too-short admin token even when the
    /// config came from a path that skipped `Config::from_env` validation (an
    /// unauthenticated admin is worse than a failed start).
    pub fn new(config: &Config, db: PgPool) -> Result<Self, String> {
        if config.admin_token.expose().len() < config::MIN_ADMIN_TOKEN_LEN {
            return Err(format!(
                "the admin token is required: at least {} bytes",
                config::MIN_ADMIN_TOKEN_LEN
            ));
        }
        Ok(Self {
            db,
            admin_token: Arc::new(config.admin_token.clone()),
            gate: Arc::new(Semaphore::new(config.max_in_flight)),
        })
    }
}

/// The router with the whole middleware stack.
pub fn app(config: &Config, state: AppState) -> Router {
    let v1 = Router::new()
        .route("/projects/{project}/desired", put(handlers::put_desired))
        .route("/projects/{project}", get(handlers::project_status))
        .route("/operations/{id}", get(handlers::get_operation))
        .route("/operations/{id}/retry", post(handlers::retry_operation))
        .route("/operation-counts", get(handlers::operation_counts))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_admin,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.gate.clone(),
            middleware::shed_load,
        ));
    let router = Router::new()
        .route("/livez", get(handlers::livez))
        .route("/readyz", get(handlers::readyz))
        .nest("/v1", v1)
        .with_state(state);
    middleware::apply(router, config)
}
