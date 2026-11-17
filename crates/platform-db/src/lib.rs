//! The Backend Platform's control store on PostgreSQL.
//!
//! - [`desired`]: desired-state writes and their admission (bounds, idempotency), and the
//!   derived project status.
//! - [`log`]: the operation log the reconciler works from — claim under a lease, fenced
//!   step saves, finish, retry.
//! - [`pool`]: the bounded pool with per-statement deadlines.
//!
//! No transaction here ever spans a provider call: the worker commits intent (a begun
//! attempt) before it calls a provider, and records the result in a later statement.

pub mod desired;
pub mod error;
pub mod log;
pub mod pool;
mod rows;
#[cfg(feature = "test-support")]
pub mod testing;

pub use desired::{
    Admitted, MAX_IN_FLIGHT_PER_PROJECT, ObservedRow, ProjectStatus, project_status, put_desired,
};
pub use error::{AdmitError, DbError};
pub use log::{Claimed, Lease, OperationCounts, OperationSummary, RetryError, StepView};
pub use pool::{PoolSettings, connect_lazy, connect_options};

/// The embedded migrations.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
