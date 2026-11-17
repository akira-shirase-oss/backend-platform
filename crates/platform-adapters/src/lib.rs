//! Provider adapters: the only code that talks to providers, linked only by the worker
//! (D-013, checked by `scripts/check-architecture.sh`).
//!
//! B0 has one adapter, [`HttpProvider`], which speaks the fake provider's protocol
//! ([`wire`]). Real adapters (PostgreSQL, the S3-compatible store, the OIDC provider)
//! arrive in B2, one at a time, each behind this same trait and each only after the
//! partial-provisioning drill runs against it.

use std::future::Future;

use platform_domain::{
    CreateRequest, ErrorClass, Observation, ProviderName, ProviderResourceId, ResourceKind,
};
use thiserror::Error;

pub mod http;
pub mod wire;

pub use http::{HttpProvider, HttpProviderConfig};

/// A provider call failure. Only [`ProviderError::Rejected`] is final: every other kind
/// may have taken effect, so the reconciler observes before trying again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ProviderError {
    #[error("provider unavailable")]
    Unavailable,
    #[error("provider call timed out")]
    Timeout,
    #[error("provider reply could not be understood")]
    Protocol,
    #[error("provider refused the request")]
    Rejected,
    #[error("the name is already taken")]
    AlreadyExists,
}

impl ProviderError {
    /// The domain's classification.
    pub fn class(self) -> ErrorClass {
        match self {
            Self::Unavailable => ErrorClass::Unavailable,
            Self::Timeout => ErrorClass::Timeout,
            Self::Protocol => ErrorClass::Protocol,
            Self::Rejected => ErrorClass::Rejected,
            Self::AlreadyExists => ErrorClass::AlreadyExists,
        }
    }
}

/// A provider of one or more resource kinds. Both calls must be safe to repeat: `lookup`
/// has no effect, and `create` is only ever sent after a lookup saw the name absent.
pub trait Provider: Send + Sync {
    /// Looks the deterministic name up.
    fn lookup(
        &self,
        kind: ResourceKind,
        name: &ProviderName,
    ) -> impl Future<Output = Result<Observation, ProviderError>> + Send;

    /// Creates the resource under its deterministic name, tagged with the operation.
    fn create(
        &self,
        request: &CreateRequest,
    ) -> impl Future<Output = Result<ProviderResourceId, ProviderError>> + Send;
}
