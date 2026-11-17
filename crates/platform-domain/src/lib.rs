//! The Backend Platform's contract in pure form.
//!
//! Everything here is deterministic and free of I/O, so the control API, the reconciler
//! worker, the fake provider and the drills share one definition of what a project may
//! ask for, what a provider resource is called, and how a provisioning operation moves.
//! `scripts/check-architecture.sh` keeps it that way.
//!
//! - [`ids`]: project ids, logical names, operation and provider resource ids.
//! - [`resource`]: resource kinds, specs and the deterministic provider names (D-016).
//! - [`plan`]: desired state → an ordered provisioning plan, with its bounds.
//! - [`operation`]: the operation and step state machine; observation before every create.
//! - [`retry`]: the per-round attempt budget and full-jitter backoff.

pub mod ids;
pub mod operation;
pub mod plan;
pub mod resource;
pub mod retry;

pub use ids::{IdError, LogicalName, OperationId, ProjectId, ProviderResourceId};
pub use operation::{
    Action, Begin, BlockReason, CreateRequest, ErrorClass, Next, Observation, ObservedResource,
    Operation, OperationOutcome, OperationState, Step, StepState, TransitionError,
};
pub use plan::{DesiredState, IdentitySpec, Plan, PlanError, PlannedResource};
pub use resource::{ProviderName, ResourceKind, ResourceSpec, provider_name};
pub use retry::RetryPolicy;
