//! Storage errors: unavailability (retry, or answer 503) is kept apart from everything else.

use thiserror::Error;

/// A storage failure.
#[derive(Debug, Error)]
pub enum DbError {
    /// The database could not answer in time (down, pool exhausted, statement or lock
    /// deadline). Callers answer `503 control_plane_unavailable` or back off and retry.
    #[error("the control database is unavailable")]
    Unavailable(#[source] sqlx::Error),
    /// A stored row breaks the domain's rules.
    #[error("stored data is invalid: {0}")]
    Corrupt(String),
    /// Anything else.
    #[error("control database error")]
    Other(#[source] sqlx::Error),
}

impl From<sqlx::Error> for DbError {
    fn from(error: sqlx::Error) -> Self {
        let unavailable = match &error {
            sqlx::Error::PoolTimedOut
            | sqlx::Error::PoolClosed
            | sqlx::Error::Io(_)
            | sqlx::Error::Tls(_)
            | sqlx::Error::Protocol(_)
            | sqlx::Error::WorkerCrashed => true,
            // 57014 query_canceled (statement_timeout), 55P03 lock_not_available,
            // 57P01 admin_shutdown, 40001 serialization_failure, 40P01 deadlock_detected.
            sqlx::Error::Database(db) => matches!(
                db.code().as_deref(),
                Some("57014" | "55P03" | "57P01" | "40001" | "40P01")
            ),
            _ => false,
        };
        if unavailable {
            Self::Unavailable(error)
        } else {
            Self::Other(error)
        }
    }
}

impl DbError {
    /// True when retrying later may succeed.
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

/// Why a desired-state write was not admitted.
#[derive(Debug, Error)]
pub enum AdmitError {
    /// The project already has the most unfinished operations allowed (D-017: a cap on
    /// in-flight platform operations, refused at admission).
    #[error("project has {in_flight} unfinished operations; the limit is {limit}")]
    TooManyOperations { in_flight: i64, limit: i64 },
    /// The idempotency key was used before with a different desired state.
    #[error("idempotency key reused with a different desired state")]
    IdempotencyConflict,
    /// Storage failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<sqlx::Error> for AdmitError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

/// True for a unique-constraint violation (SQLSTATE 23505).
pub(crate) fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}
