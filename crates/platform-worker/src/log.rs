//! The operation-log port. The worker sees only this trait, so the drills can run the same
//! reconcile code against an in-memory log (thousands of schedules a second) and the
//! process-kill drill against PostgreSQL.

use std::future::Future;
use std::time::Duration;

use platform_db::{Claimed, DbError, Lease};
use platform_domain::{OperationState, Step};
use sqlx::PgPool;
use thiserror::Error;
use uuid::Uuid;

/// Why the log refused a write.
#[derive(Debug, Error)]
pub enum LogError {
    /// The lease is gone: another worker owns the operation now. Stop touching it.
    #[error("the lease was lost to another worker")]
    Fenced,
    /// Storage failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The control database did not answer before the client-side deadline (F12): a
    /// half-open connection the server's own timeouts cannot see. Whether the write
    /// committed is unknown; the next claim reads what was recorded.
    #[error("the control database did not answer within {0:?}")]
    Deadline(Duration),
}

/// Where operations and their recorded steps live.
pub trait OperationLog: Send + Sync {
    /// Claims the next runnable operation.
    fn claim(
        &self,
        worker: Uuid,
        lease_for: Duration,
    ) -> impl Future<Output = Result<Option<Claimed>, LogError>> + Send;

    /// Saves steps and renews the lease; [`LogError::Fenced`] when the lease is gone.
    fn save(
        &self,
        lease: &Lease,
        steps: &[&Step],
        lease_for: Duration,
    ) -> impl Future<Output = Result<(), LogError>> + Send;

    /// Saves the last steps and ends the round, releasing the lease.
    fn finish(
        &self,
        lease: &Lease,
        state: OperationState,
        steps: &[&Step],
    ) -> impl Future<Output = Result<(), LogError>> + Send;
}

/// Default client-side deadline on each log call: longer than the pool's acquire timeout
/// (1 s) plus the server's `statement_timeout` (2 s), so it only fires when neither can.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(5);

/// The PostgreSQL operation log. Every call has a client-side deadline.
#[derive(Debug, Clone)]
pub struct PgLog {
    pool: PgPool,
    deadline: Duration,
}

impl PgLog {
    /// Wraps a pool with [`DEFAULT_DEADLINE`].
    pub fn new(pool: PgPool) -> Self {
        Self::with_deadline(pool, DEFAULT_DEADLINE)
    }

    /// Wraps a pool with a specific deadline per call.
    pub fn with_deadline(pool: PgPool, deadline: Duration) -> Self {
        Self { pool, deadline }
    }

    async fn bounded<T>(
        &self,
        call: impl Future<Output = Result<T, DbError>>,
    ) -> Result<T, LogError> {
        match tokio::time::timeout(self.deadline, call).await {
            Ok(result) => Ok(result?),
            Err(_) => Err(LogError::Deadline(self.deadline)),
        }
    }
}

impl OperationLog for PgLog {
    async fn claim(&self, worker: Uuid, lease_for: Duration) -> Result<Option<Claimed>, LogError> {
        self.bounded(platform_db::log::claim(&self.pool, worker, lease_for))
            .await
    }

    async fn save(
        &self,
        lease: &Lease,
        steps: &[&Step],
        lease_for: Duration,
    ) -> Result<(), LogError> {
        if self
            .bounded(platform_db::log::save_steps(
                &self.pool, lease, steps, lease_for,
            ))
            .await?
        {
            Ok(())
        } else {
            Err(LogError::Fenced)
        }
    }

    async fn finish(
        &self,
        lease: &Lease,
        state: OperationState,
        steps: &[&Step],
    ) -> Result<(), LogError> {
        if self
            .bounded(platform_db::log::finish(&self.pool, lease, state, steps))
            .await?
        {
            Ok(())
        } else {
            Err(LogError::Fenced)
        }
    }
}
