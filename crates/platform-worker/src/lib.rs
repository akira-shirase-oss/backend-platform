//! The reconciler: claim an operation under a lease, run its recorded steps through a
//! provider adapter, record every result, resume after any crash.
//!
//! - [`log`]: the operation-log port and its PostgreSQL implementation.
//! - [`reconcile`]: one claimed operation, step by step — the code the drills exercise.
//! - [`worker`]: the bounded pool of reconcile tasks, with shutdown.
//! - [`config`]: typed, validated configuration from the environment.

pub mod config;
pub mod log;
pub mod reconcile;
pub mod worker;

pub use log::{LogError, OperationLog, PgLog};
pub use reconcile::{Halted, Hooks, NoHooks, Point, ReconcileConfig, RunError, run_claimed};
