//! The fake provider's HTTP protocol (`/v1`), shared by the fixture and [`crate::HttpProvider`].
//!
//! - `GET  /v1/{kind}/{name}` → `200` [`LookupBody`] (every resource holding the name) or
//!   `404`
//! - `POST /v1/{kind}` [`CreateBody`] → `201` [`CreatedBody`], `409` (name taken, only
//!   when the fake enforces unique names), `400` (rejected), `503` (unavailable)

use platform_domain::{OperationId, ProviderName, ProviderResourceId, ResourceKind, ResourceSpec};
use serde::{Deserialize, Serialize};

/// Largest reply body an adapter reads.
pub const MAX_REPLY_BYTES: usize = 16 * 1024;

/// A resource as the provider reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceBody {
    /// The provider id.
    pub id: ProviderResourceId,
    /// What it looks like.
    pub spec: ResourceSpec,
    /// The creating operation.
    pub tag: Option<OperationId>,
}

/// A lookup reply: every resource under the name. More than one is a duplicate, reported
/// as such, never collapsed to the first (F5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupBody {
    /// The matches.
    pub matches: Vec<ResourceBody>,
}

/// A create request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBody {
    /// The deterministic name.
    pub name: ProviderName,
    /// What to create.
    pub spec: ResourceSpec,
    /// The creating operation.
    pub tag: OperationId,
    /// De-duplication key; a provider that supports keys returns the first resource made
    /// with it instead of making another. Empty means none.
    #[serde(default)]
    pub idempotency_key: String,
}

/// A create reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatedBody {
    /// The provider id.
    pub id: ProviderResourceId,
}

/// `GET` path for a name.
pub fn lookup_path(kind: ResourceKind, name: &ProviderName) -> String {
    format!("/v1/{}/{}", kind.as_str(), name.as_str())
}

/// `POST` path for a kind.
pub fn create_path(kind: ResourceKind) -> String {
    format!("/v1/{}", kind.as_str())
}
