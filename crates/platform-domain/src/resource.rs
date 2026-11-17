//! Resource kinds, the spec a provider resource must match, and the deterministic names
//! that make a retry find what already exists instead of creating a second one (D-016).

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{LogicalName, ProjectId};

/// What kind of provider resource a step provisions. Each kind is its own provider-side
/// namespace (PostgreSQL keeps roles and databases apart; buckets and realms are separate
/// services).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// The PostgreSQL role that owns one project database (D-014).
    DatabaseRole,
    /// A PostgreSQL database.
    Database,
    /// An S3-compatible bucket.
    Bucket,
    /// The project's OIDC realm (one per project, D-019).
    OidcRealm,
    /// An OIDC client inside the project's realm.
    OidcClient,
}

impl ResourceKind {
    /// Every kind, in a stable order.
    pub const ALL: [Self; 5] = [
        Self::DatabaseRole,
        Self::Database,
        Self::Bucket,
        Self::OidcRealm,
        Self::OidcClient,
    ];

    /// The wire and storage spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DatabaseRole => "database_role",
            Self::Database => "database",
            Self::Bucket => "bucket",
            Self::OidcRealm => "oidc_realm",
            Self::OidcClient => "oidc_client",
        }
    }

    /// Parses the wire spelling.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Longest provider name: PostgreSQL's identifier limit (63 bytes) and S3's bucket-name
/// limit (63 characters).
pub const MAX_PROVIDER_NAME_LEN: usize = 63;

/// A provider-side name. Built only by [`provider_name`] from validated parts, or parsed
/// back (from storage or a provider reply) with the same character rules.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderName(String);

/// A provider name that breaks the character or length rules.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("provider name must be 3 to 63 of [a-z0-9_-], starting with a letter")]
pub struct ProviderNameError;

impl ProviderName {
    /// Parses a stored or reported name.
    pub fn parse(value: &str) -> Result<Self, ProviderNameError> {
        let ok = (3..=MAX_PROVIDER_NAME_LEN).contains(&value.len())
            && value.starts_with(|c: char| c.is_ascii_lowercase())
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        if ok {
            Ok(Self(value.to_owned()))
        } else {
            Err(ProviderNameError)
        }
    }

    /// The name as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProviderName {
    type Error = ProviderNameError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ProviderName> for String {
    fn from(value: ProviderName) -> Self {
        value.0
    }
}

impl fmt::Display for ProviderName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The deterministic provider name of a project's resource (D-016): a pure function of
/// the project id, the kind and the logical name, so a retried, resumed or restored
/// operation looks up the same name and adopts what it finds.
///
/// - `database_role`: `p_<project>_<name>_owner`
/// - `database`: `p_<project>_<name>`
/// - `bucket`: `p-<project>-<name with _ as ->` (S3 names allow no `_`)
/// - `oidc_realm`, `oidc_client`: `p-<project>-<name>`
///
/// Unambiguous within a kind: project ids contain neither `_` nor `-`, so the first
/// separator after the `p` prefix ends the project id, and logical names contain no `-`,
/// so the bucket mapping is reversible. The longest result is 53 bytes.
pub fn provider_name(project: &ProjectId, kind: ResourceKind, name: &LogicalName) -> ProviderName {
    let project = project.as_str();
    let name = name.as_str();
    let rendered = match kind {
        ResourceKind::DatabaseRole => format!("p_{project}_{name}_owner"),
        ResourceKind::Database => format!("p_{project}_{name}"),
        ResourceKind::Bucket => format!("p-{project}-{}", name.replace('_', "-")),
        ResourceKind::OidcRealm | ResourceKind::OidcClient => format!("p-{project}-{name}"),
    };
    ProviderName(rendered)
}

/// What a resource must look like. An existing resource under the deterministic name is
/// adopted only when its observed spec equals the desired one; otherwise the step is
/// blocked as a conflict for a person to look at, never overwritten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceSpec {
    /// A `LOGIN` role with a connection limit (D-014).
    DatabaseRole { connection_limit: u16 },
    /// A database owned by the named role.
    Database { owner: ProviderName },
    /// A bucket with an enforced quota (D-017).
    Bucket { quota_bytes: u64 },
    /// The project's realm.
    OidcRealm,
    /// A client inside the named realm.
    OidcClient { realm: ProviderName },
}

impl ResourceSpec {
    /// The kind this spec belongs to.
    pub fn kind(&self) -> ResourceKind {
        match self {
            Self::DatabaseRole { .. } => ResourceKind::DatabaseRole,
            Self::Database { .. } => ResourceKind::Database,
            Self::Bucket { .. } => ResourceKind::Bucket,
            Self::OidcRealm => ResourceKind::OidcRealm,
            Self::OidcClient { .. } => ResourceKind::OidcClient,
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn project() -> impl Strategy<Value = ProjectId> {
        "[a-z][a-z0-9]{2,19}".prop_map(|s| ProjectId::parse(&s).unwrap())
    }

    fn logical() -> impl Strategy<Value = LogicalName> {
        "[a-z][a-z0-9]{0,6}(_[a-z0-9]{1,5}){0,3}"
            .prop_filter_map("too long", |s| LogicalName::parse(&s).ok())
    }

    fn kind() -> impl Strategy<Value = ResourceKind> {
        prop::sample::select(ResourceKind::ALL.to_vec())
    }

    #[test]
    fn examples() {
        let p = ProjectId::parse("shop").unwrap();
        let n = LogicalName::parse("user_files").unwrap();
        assert_eq!(
            provider_name(&p, ResourceKind::DatabaseRole, &n).as_str(),
            "p_shop_user_files_owner"
        );
        assert_eq!(
            provider_name(&p, ResourceKind::Database, &n).as_str(),
            "p_shop_user_files"
        );
        assert_eq!(
            provider_name(&p, ResourceKind::Bucket, &n).as_str(),
            "p-shop-user-files"
        );
        assert_eq!(
            provider_name(&p, ResourceKind::OidcRealm, &n).as_str(),
            "p-shop-user_files"
        );
    }

    #[test]
    fn kinds_round_trip() {
        for kind in ResourceKind::ALL {
            assert_eq!(ResourceKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(ResourceKind::parse("table"), None);
    }

    #[test]
    fn longest_name_fits_every_provider_limit() {
        let p = ProjectId::parse(&"a".repeat(20)).unwrap();
        let n = LogicalName::parse(&"b".repeat(24)).unwrap();
        for kind in ResourceKind::ALL {
            let name = provider_name(&p, kind, &n);
            assert!(name.as_str().len() <= 53, "{kind}: {name}");
            assert!(ProviderName::parse(name.as_str()).is_ok());
        }
    }

    proptest! {
        /// The property behind "a retry finds what exists": the same inputs always give
        /// the same name, and different inputs never collide within a kind.
        #[test]
        fn names_are_deterministic_and_injective(
            p1 in project(), n1 in logical(), p2 in project(), n2 in logical(), k in kind(),
        ) {
            prop_assert_eq!(provider_name(&p1, k, &n1), provider_name(&p1, k, &n1));
            if (&p1, &n1) != (&p2, &n2) {
                prop_assert_ne!(provider_name(&p1, k, &n1), provider_name(&p2, k, &n2));
            }
        }

        #[test]
        fn names_always_parse_back(p in project(), n in logical(), k in kind()) {
            let name = provider_name(&p, k, &n);
            prop_assert!(ProviderName::parse(name.as_str()).is_ok());
            if k == ResourceKind::Bucket {
                // S3: 3–63 characters, lowercase letters, digits and hyphens, letter or
                // digit at both ends.
                prop_assert!(!name.as_str().contains('_'));
                prop_assert!(name.as_str().ends_with(|c: char| c.is_ascii_alphanumeric()));
            }
        }
    }
}
