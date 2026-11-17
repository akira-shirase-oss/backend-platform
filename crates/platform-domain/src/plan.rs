//! Desired state → an ordered provisioning plan.
//!
//! The plan is the operation's recorded intent: every step carries its deterministic
//! provider name and spec, so a worker that resumes an operation after a crash needs
//! nothing but the stored plan to know exactly which resources may exist.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{LogicalName, ProjectId};
use crate::resource::{ProviderName, ResourceKind, ResourceSpec, provider_name};

/// Most resources one project's desired state may name (a bounded plan, AGENTS.md rule 6).
pub const MAX_RESOURCES_PER_PROJECT: usize = 16;
/// Connection limit given to each project database role (D-014; a proposed lab value).
pub const DEFAULT_CONNECTION_LIMIT: u16 = 20;
/// Quota given to each bucket (D-017; a proposed lab value): 1 GiB.
pub const DEFAULT_BUCKET_QUOTA_BYTES: u64 = 1 << 30;
/// The logical name of the project's single realm (D-019: one realm per project).
pub const REALM_NAME: &str = "realm";

/// What a project asks for. Unknown fields are refused, not ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredState {
    /// Databases, each with its own owner role.
    #[serde(default)]
    pub databases: Vec<LogicalName>,
    /// Buckets.
    #[serde(default)]
    pub buckets: Vec<LogicalName>,
    /// The project's identity realm and its clients, when wanted.
    #[serde(default)]
    pub identity: Option<IdentitySpec>,
}

/// The project's realm and the OIDC clients inside it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentitySpec {
    /// Client names.
    #[serde(default)]
    pub clients: Vec<LogicalName>,
}

/// One resource of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedResource {
    /// Its kind.
    pub kind: ResourceKind,
    /// Its name inside the project.
    pub logical: LogicalName,
    /// Its deterministic provider name.
    pub name: ProviderName,
    /// What it must look like.
    pub spec: ResourceSpec,
}

/// A validated, ordered plan: every dependency precedes its dependants (a database's
/// owner role before the database, the realm before its clients).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The project.
    pub project: ProjectId,
    /// The resources, in execution order.
    pub resources: Vec<PlannedResource>,
}

/// Why a desired state was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlanError {
    #[error("the desired state names no resources")]
    Empty,
    #[error("the desired state names {count} resources; the limit is {MAX_RESOURCES_PER_PROJECT}")]
    TooMany { count: usize },
    #[error("{kind} `{name}` is named twice")]
    Duplicate { kind: ResourceKind, name: String },
}

impl Plan {
    /// Builds the plan for `project`'s desired state.
    pub fn build(project: &ProjectId, desired: &DesiredState) -> Result<Self, PlanError> {
        let mut resources = Vec::new();
        let mut push = |kind: ResourceKind, logical: &LogicalName, spec: ResourceSpec| {
            resources.push(PlannedResource {
                kind,
                logical: logical.clone(),
                name: provider_name(project, kind, logical),
                spec,
            });
        };
        reject_duplicates(ResourceKind::Database, &desired.databases)?;
        reject_duplicates(ResourceKind::Bucket, &desired.buckets)?;
        for db in &desired.databases {
            let role = provider_name(project, ResourceKind::DatabaseRole, db);
            push(
                ResourceKind::DatabaseRole,
                db,
                ResourceSpec::DatabaseRole {
                    connection_limit: DEFAULT_CONNECTION_LIMIT,
                },
            );
            push(
                ResourceKind::Database,
                db,
                ResourceSpec::Database { owner: role },
            );
        }
        for bucket in &desired.buckets {
            push(
                ResourceKind::Bucket,
                bucket,
                ResourceSpec::Bucket {
                    quota_bytes: DEFAULT_BUCKET_QUOTA_BYTES,
                },
            );
        }
        if let Some(identity) = &desired.identity {
            reject_duplicates(ResourceKind::OidcClient, &identity.clients)?;
            #[expect(clippy::expect_used, reason = "REALM_NAME is a constant valid name")]
            let realm_logical =
                LogicalName::parse(REALM_NAME).expect("invariant: REALM_NAME is a valid name");
            let realm = provider_name(project, ResourceKind::OidcRealm, &realm_logical);
            push(
                ResourceKind::OidcRealm,
                &realm_logical,
                ResourceSpec::OidcRealm,
            );
            for client in &identity.clients {
                push(
                    ResourceKind::OidcClient,
                    client,
                    ResourceSpec::OidcClient {
                        realm: realm.clone(),
                    },
                );
            }
        }
        if resources.is_empty() {
            return Err(PlanError::Empty);
        }
        if resources.len() > MAX_RESOURCES_PER_PROJECT {
            return Err(PlanError::TooMany {
                count: resources.len(),
            });
        }
        Ok(Self {
            project: project.clone(),
            resources,
        })
    }
}

fn reject_duplicates(kind: ResourceKind, names: &[LogicalName]) -> Result<(), PlanError> {
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(PlanError::Duplicate {
                kind,
                name: name.to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<LogicalName> {
        list.iter()
            .map(|n| LogicalName::parse(n).unwrap())
            .collect()
    }

    #[test]
    fn the_starter_plan_orders_dependencies_first() {
        let project = ProjectId::parse("shop").unwrap();
        let desired = DesiredState {
            databases: names(&["main"]),
            buckets: names(&["files"]),
            identity: Some(IdentitySpec {
                clients: names(&["web"]),
            }),
        };
        let plan = Plan::build(&project, &desired).unwrap();
        let rendered: Vec<_> = plan
            .resources
            .iter()
            .map(|r| format!("{}:{}", r.kind, r.name))
            .collect();
        assert_eq!(
            rendered,
            [
                "database_role:p_shop_main_owner",
                "database:p_shop_main",
                "bucket:p-shop-files",
                "oidc_realm:p-shop-realm",
                "oidc_client:p-shop-web",
            ]
        );
        assert_eq!(
            plan.resources[1].spec,
            ResourceSpec::Database {
                owner: plan.resources[0].name.clone()
            }
        );
    }

    #[test]
    fn plans_are_bounded_and_unambiguous() {
        let project = ProjectId::parse("shop").unwrap();
        assert_eq!(
            Plan::build(&project, &DesiredState::default()),
            Err(PlanError::Empty)
        );
        let dup = DesiredState {
            buckets: names(&["a", "a"]),
            ..DesiredState::default()
        };
        assert!(matches!(
            Plan::build(&project, &dup),
            Err(PlanError::Duplicate { .. })
        ));
        let many = DesiredState {
            buckets: (0..17)
                .map(|i| LogicalName::parse(&format!("b{i}")).unwrap())
                .collect(),
            ..DesiredState::default()
        };
        assert_eq!(
            Plan::build(&project, &many),
            Err(PlanError::TooMany { count: 17 })
        );
    }

    #[test]
    fn unknown_fields_are_refused() {
        let parsed: Result<DesiredState, _> =
            serde_json::from_str(r#"{"databases":["main"],"cache":["x"]}"#);
        assert!(parsed.is_err());
        let parsed: Result<DesiredState, _> = serde_json::from_str(r#"{"databases":["Main"]}"#);
        assert!(parsed.is_err(), "names are validated while parsing");
    }
}
