//! The provisioning operation and its steps: a reconciled process, not a transaction (D-003).
//!
//! The rule that prevents duplicates lives in [`Step::observe`]: an attempt never creates
//! without first observing the deterministic name, and any attempt whose outcome was not
//! recorded leaves the step [`StepState::Uncertain`] — so the next attempt, by this worker
//! or one that resumed after a crash, observes again and adopts what a lost reply left
//! behind. Every failure is treated as "maybe it happened"; that is what makes the retry
//! safe without knowing which failure it was.
//!
//! Lifecycle of a step within one retry round:
//!
//! ```text
//! Pending ──begin──▶ Uncertain ──observe: absent──▶ (create) ──created + verified──▶ Done
//!                       ▲  │ └──observe: present, same spec──────────────▶ Done (adopted)
//!                       │  ├──observe: present, other spec──▶ Blocked (conflict)
//!                       │  ├──observe or verify: two holders──▶ Blocked (duplicate)
//!                       │  ├──failed: rejected──────────────▶ Blocked (rejected)
//!                       └──┴──failed: retryable / crash──▶ Uncertain ──budget spent──▶ GaveUp
//! ```
//!
//! An operation is `succeeded` when every step is `Done`, and `partial` when a step is
//! `Blocked` or `GaveUp` — shown with the resource ids it did record, and retryable.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{OperationId, ProjectId, ProviderResourceId};
use crate::plan::{Plan, PlannedResource};
use crate::resource::{ProviderName, ResourceKind, ResourceSpec};
use crate::retry::RetryPolicy;

/// What a provider reports under a deterministic name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedResource {
    /// The provider's id.
    pub id: ProviderResourceId,
    /// What the resource actually looks like.
    pub spec: ResourceSpec,
    /// The operation that created it, when the provider kept the tag.
    pub tag: Option<OperationId>,
}

/// The result of looking a name up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "observed", rename_all = "snake_case")]
pub enum Observation {
    /// Nothing exists under the name.
    Absent,
    /// A resource exists under the name.
    Present(ObservedResource),
    /// More than one resource holds the name: a duplicate made outside the platform's
    /// control (or by a provider that took a create late). Never adopted (F5).
    Many { found: Vec<ObservedResource> },
}

impl Observation {
    /// Builds an observation from every resource a provider holds under one name.
    pub fn from_matches(mut found: Vec<ObservedResource>) -> Self {
        match found.len() {
            0 => Self::Absent,
            1 => Self::Present(found.remove(0)),
            _ => Self::Many { found },
        }
    }
}

/// A provider failure, classified. Only `Rejected` is final; every other class leaves the
/// outcome unknown, so the step stays uncertain and the next attempt observes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    /// The provider answered that it cannot serve now (or the connection failed).
    Unavailable,
    /// No answer before the deadline; the call may have taken effect.
    Timeout,
    /// The answer could not be understood; the call may have taken effect.
    Protocol,
    /// The provider refused the request as invalid; retrying cannot help.
    Rejected,
    /// A create found the name taken: somebody (perhaps a lost earlier attempt) made it.
    AlreadyExists,
}

impl ErrorClass {
    /// True unless retrying cannot change the answer.
    pub fn is_retryable(self) -> bool {
        !matches!(self, Self::Rejected)
    }

    /// The storage and wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Protocol => "protocol",
            Self::Rejected => "rejected",
            Self::AlreadyExists => "already_exists",
        }
    }
}

/// Why a step is blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    /// A resource with the deterministic name exists but does not match the spec. It is
    /// never overwritten or replaced; a person decides.
    Conflict,
    /// The provider refused the request.
    Rejected,
    /// The provider holds more than one resource under the deterministic name. Both are
    /// left as found; a person decides which one is the project's.
    Duplicate,
}

/// Where a step stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StepState {
    /// Not attempted in this round.
    Pending,
    /// An attempt began and its outcome is not recorded: the provider may or may not hold
    /// the resource. The next attempt observes before anything else.
    Uncertain,
    /// The resource exists with this provider id.
    Done {
        resource_id: ProviderResourceId,
        /// True when an existing resource was found and adopted rather than created.
        adopted: bool,
    },
    /// The step cannot proceed without a person.
    Blocked { reason: BlockReason },
    /// The round's attempt budget ran out with the outcome still uncertain.
    GaveUp,
}

impl StepState {
    /// The derived status shown to people: `pending`, `uncertain`, `ready`, `blocked` or
    /// `gave_up`. Never smoothed over (invariant 1).
    pub fn status(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Uncertain => "uncertain",
            Self::Done { .. } => "ready",
            Self::Blocked { .. } => "blocked",
            Self::GaveUp => "gave_up",
        }
    }
}

/// A transition the state machine refuses: a bug in the caller, never input-driven.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TransitionError {
    #[error("step {position} cannot {action} while {state}")]
    Step {
        position: u16,
        action: &'static str,
        state: &'static str,
    },
    #[error("operation cannot {action} while {state}")]
    Operation {
        action: &'static str,
        state: &'static str,
    },
}

/// A create the worker may send: produced only by [`Step::observe`] seeing `Absent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRequest {
    /// The resource kind.
    pub kind: ResourceKind,
    /// The deterministic name.
    pub name: ProviderName,
    /// What to create.
    pub spec: ResourceSpec,
    /// The creating operation, written onto the resource where the provider supports it.
    pub tag: OperationId,
    /// Sent to providers that de-duplicate creates by key: `<operation>/<position>`, the
    /// same for every attempt, round and worker of this step, so a stale create from a
    /// paused or slow attempt is recognised as the same create (D-029).
    pub idempotency_key: String,
}

/// What the worker does after an observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing exists: send this create.
    Create(CreateRequest),
    /// The step is settled (adopted or blocked); record it.
    Settled,
}

/// The result of starting an attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Begin {
    /// Attempt number `n` (1-based, within the round) may run.
    Attempt(u16),
    /// The budget is spent; the step is now `GaveUp`.
    Exhausted,
}

/// One step of an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// Its position in the plan.
    pub position: u16,
    /// What it provisions.
    pub resource: PlannedResource,
    /// Where it stands.
    pub state: StepState,
    /// Attempts begun in the current round.
    pub attempts: u16,
    /// The last failure's class (a code, never a provider message).
    pub last_error: Option<ErrorClass>,
}

impl Step {
    /// A pending step.
    pub fn new(position: u16, resource: PlannedResource) -> Self {
        Self {
            position,
            resource,
            state: StepState::Pending,
            attempts: 0,
            last_error: None,
        }
    }

    fn refuse(&self, action: &'static str) -> TransitionError {
        TransitionError::Step {
            position: self.position,
            action,
            state: self.state.status(),
        }
    }

    /// Starts an attempt. The worker records the result *before* calling a provider, so
    /// a crash mid-attempt still counts against the budget and leaves the step uncertain.
    pub fn begin_attempt(&mut self, policy: &RetryPolicy) -> Result<Begin, TransitionError> {
        match self.state {
            StepState::Pending | StepState::Uncertain => {}
            _ => return Err(self.refuse("begin an attempt")),
        }
        if self.attempts >= policy.max_attempts {
            self.state = StepState::GaveUp;
            return Ok(Begin::Exhausted);
        }
        self.attempts += 1;
        self.state = StepState::Uncertain;
        Ok(Begin::Attempt(self.attempts))
    }

    /// Applies an observation of the deterministic name: adopt a matching resource, block
    /// on a mismatching one, or ask for a create when nothing exists.
    pub fn observe(
        &mut self,
        observation: &Observation,
        operation: OperationId,
    ) -> Result<Action, TransitionError> {
        if self.state != StepState::Uncertain {
            return Err(self.refuse("observe"));
        }
        match observation {
            Observation::Absent => Ok(Action::Create(CreateRequest {
                kind: self.resource.kind,
                name: self.resource.name.clone(),
                spec: self.resource.spec.clone(),
                tag: operation,
                idempotency_key: format!("{operation}/{}", self.position),
            })),
            Observation::Present(found) if found.spec == self.resource.spec => {
                self.state = StepState::Done {
                    resource_id: found.id.clone(),
                    adopted: true,
                };
                Ok(Action::Settled)
            }
            Observation::Present(_) => {
                self.state = StepState::Blocked {
                    reason: BlockReason::Conflict,
                };
                Ok(Action::Settled)
            }
            Observation::Many { .. } => {
                self.state = StepState::Blocked {
                    reason: BlockReason::Duplicate,
                };
                Ok(Action::Settled)
            }
        }
    }

    /// Records a create the provider confirmed.
    pub fn created(&mut self, id: ProviderResourceId) -> Result<(), TransitionError> {
        if self.state != StepState::Uncertain {
            return Err(self.refuse("record a create"));
        }
        self.state = StepState::Done {
            resource_id: id,
            adopted: false,
        };
        self.last_error = None;
        Ok(())
    }

    /// Checks a resource just created, or adopted after an attempt whose outcome was unknown,
    /// against a fresh lookup of its name (F5). A create that landed late, or any other
    /// second holder, shows up here and blocks the step as a duplicate; an answer that does
    /// not show the recorded id puts the step back to uncertain, to be observed again.
    /// Never reports success with an extra resource.
    pub fn verify(&mut self, observation: &Observation) -> Result<(), TransitionError> {
        let StepState::Done { resource_id, .. } = &self.state else {
            return Err(self.refuse("verify a resource"));
        };
        match observation {
            Observation::Present(found) if &found.id == resource_id => {}
            Observation::Many { .. } => {
                self.state = StepState::Blocked {
                    reason: BlockReason::Duplicate,
                };
            }
            Observation::Absent | Observation::Present(_) => {
                self.state = StepState::Uncertain;
                self.last_error = Some(ErrorClass::Protocol);
            }
        }
        Ok(())
    }

    /// The verifying lookup failed: the create stands unverified, so the step goes back to
    /// uncertain and the next attempt observes again.
    pub fn verification_failed(&mut self, class: ErrorClass) -> Result<(), TransitionError> {
        if !matches!(self.state, StepState::Done { .. }) {
            return Err(self.refuse("record a failed verification"));
        }
        self.state = StepState::Uncertain;
        self.last_error = Some(class);
        Ok(())
    }

    /// Records a failed call. Retryable classes keep the step uncertain.
    pub fn failed(&mut self, class: ErrorClass) -> Result<(), TransitionError> {
        if self.state != StepState::Uncertain {
            return Err(self.refuse("record a failure"));
        }
        self.last_error = Some(class);
        if !class.is_retryable() {
            self.state = StepState::Blocked {
                reason: BlockReason::Rejected,
            };
        }
        Ok(())
    }

    /// The recorded provider id, when the step is done.
    pub fn resource_id(&self) -> Option<&ProviderResourceId> {
        match &self.state {
            StepState::Done { resource_id, .. } => Some(resource_id),
            _ => None,
        }
    }
}

/// Where an operation stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    /// Waiting for a worker.
    Pending,
    /// Claimed by a worker (or by one that crashed; its lease will expire).
    Running,
    /// Every step is done.
    Succeeded,
    /// A step is blocked or gave up; the done steps keep their real resource ids.
    Partial,
}

impl OperationState {
    /// The storage and wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Partial => "partial",
        }
    }

    /// Parses the storage spelling.
    pub fn parse(value: &str) -> Option<Self> {
        [Self::Pending, Self::Running, Self::Succeeded, Self::Partial]
            .into_iter()
            .find(|s| s.as_str() == value)
    }

    /// True for `succeeded` and `partial`.
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Partial)
    }
}

impl fmt::Display for OperationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a round of an operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationOutcome {
    /// Every step is done.
    Succeeded,
    /// A step is blocked or gave up.
    Partial,
}

/// What a worker does next with an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Run an attempt of the step at this index.
    Attempt(usize),
    /// The round is over.
    Finish(OperationOutcome),
}

/// A provisioning operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    /// Its id (also the tag on what it creates).
    pub id: OperationId,
    /// The project.
    pub project: ProjectId,
    /// The desired-state revision it reconciles.
    pub revision: u64,
    /// Retry round: 0 at creation, +1 for each retry of a partial operation.
    pub round: u16,
    /// Where it stands.
    pub state: OperationState,
    /// Its steps, in plan order.
    pub steps: Vec<Step>,
}

impl Operation {
    /// A pending operation for `plan`.
    pub fn new(id: OperationId, revision: u64, plan: &Plan) -> Self {
        let steps = plan
            .resources
            .iter()
            .enumerate()
            .map(|(i, resource)| {
                #[expect(clippy::expect_used, reason = "Plan::build bounds plans to 16 steps")]
                let position = u16::try_from(i).expect("invariant: plans are bounded to 16 steps");
                Step::new(position, resource.clone())
            })
            .collect();
        Self {
            id,
            project: plan.project.clone(),
            revision,
            round: 0,
            state: OperationState::Pending,
            steps,
        }
    }

    /// The next thing to do. Steps run strictly in order; the first step that is blocked
    /// or gave up ends the round as `partial`, leaving later steps pending and visible.
    pub fn next(&self) -> Next {
        for (index, step) in self.steps.iter().enumerate() {
            match step.state {
                StepState::Done { .. } => {}
                StepState::Pending | StepState::Uncertain => return Next::Attempt(index),
                StepState::Blocked { .. } | StepState::GaveUp => {
                    return Next::Finish(OperationOutcome::Partial);
                }
            }
        }
        Next::Finish(OperationOutcome::Succeeded)
    }

    /// Ends the round.
    pub fn finish(&mut self, outcome: OperationOutcome) -> Result<(), TransitionError> {
        if self.state.is_finished() {
            return Err(TransitionError::Operation {
                action: "finish",
                state: self.state.as_str(),
            });
        }
        self.state = match outcome {
            OperationOutcome::Succeeded => OperationState::Succeeded,
            OperationOutcome::Partial => OperationState::Partial,
        };
        Ok(())
    }

    /// Starts a new round of a partial operation: blocked and gave-up steps become pending
    /// with a fresh budget, done steps keep their ids. The same operation resumes; nothing
    /// is re-created blindly, because every step observes before it creates.
    pub fn retry(&mut self) -> Result<(), TransitionError> {
        if self.state != OperationState::Partial {
            return Err(TransitionError::Operation {
                action: "retry",
                state: self.state.as_str(),
            });
        }
        self.round = self.round.saturating_add(1);
        self.state = OperationState::Pending;
        for step in &mut self.steps {
            if matches!(step.state, StepState::Blocked { .. } | StepState::GaveUp) {
                step.state = StepState::Pending;
                step.attempts = 0;
            }
        }
        Ok(())
    }

    /// The derived status: the operation state, except that a running operation with a
    /// step whose outcome is unknown says `uncertain`.
    pub fn status(&self) -> &'static str {
        if !self.state.is_finished() && self.steps.iter().any(|s| s.state == StepState::Uncertain) {
            return "uncertain";
        }
        self.state.as_str()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use uuid::Uuid;

    use super::*;
    use crate::ids::{LogicalName, ProjectId};
    use crate::plan::{DesiredState, IdentitySpec};

    fn op() -> Operation {
        let project = ProjectId::parse("shop").unwrap();
        let desired = DesiredState {
            databases: vec![LogicalName::parse("main").unwrap()],
            buckets: vec![LogicalName::parse("files").unwrap()],
            identity: Some(IdentitySpec::default()),
        };
        let plan = Plan::build(&project, &desired).unwrap();
        Operation::new(OperationId(Uuid::from_u128(7)), 1, &plan)
    }

    fn id(n: u32) -> ProviderResourceId {
        ProviderResourceId::parse(&format!("r{n}")).unwrap()
    }

    #[test]
    fn happy_path_creates_after_observing_absence() {
        let mut op = op();
        let policy = RetryPolicy::default();
        let mut n = 0;
        while let Next::Attempt(i) = op.next() {
            let tag = op.id;
            let step = &mut op.steps[i];
            assert_eq!(step.begin_attempt(&policy), Ok(Begin::Attempt(1)));
            let Action::Create(request) = step.observe(&Observation::Absent, tag).unwrap() else {
                panic!("absent must ask for a create");
            };
            assert_eq!(request.name, step.resource.name);
            assert_eq!(request.tag, tag);
            n += 1;
            step.created(id(n)).unwrap();
        }
        assert_eq!(op.next(), Next::Finish(OperationOutcome::Succeeded));
        op.finish(OperationOutcome::Succeeded).unwrap();
        assert_eq!(op.status(), "succeeded");
        assert!(op.finish(OperationOutcome::Succeeded).is_err());
    }

    #[test]
    fn the_idempotency_key_is_stable_across_attempts_and_rounds() {
        let mut op = op();
        let policy = RetryPolicy {
            max_attempts: 1,
            ..RetryPolicy::default()
        };
        let tag = op.id;
        let key = |op: &mut Operation| {
            let step = &mut op.steps[1];
            step.begin_attempt(&policy).unwrap();
            let Action::Create(request) = step.observe(&Observation::Absent, tag).unwrap() else {
                panic!("absent must ask for a create");
            };
            request.idempotency_key
        };
        op.steps[0].state = StepState::Done {
            resource_id: id(1),
            adopted: false,
        };
        let first = key(&mut op);
        assert_eq!(first, format!("{tag}/1"));
        op.steps[1].failed(ErrorClass::Timeout).unwrap();
        assert_eq!(op.steps[1].begin_attempt(&policy), Ok(Begin::Exhausted));
        op.finish(OperationOutcome::Partial).unwrap();
        op.retry().unwrap();
        assert_eq!(key(&mut op), first, "a new round sends the same key");
    }

    #[test]
    fn a_matching_resource_is_adopted_and_a_mismatch_blocks() {
        let mut op = op();
        let policy = RetryPolicy::default();
        let tag = op.id;
        let step = &mut op.steps[0];
        step.begin_attempt(&policy).unwrap();
        let found = ObservedResource {
            id: id(1),
            spec: step.resource.spec.clone(),
            tag: None,
        };
        assert_eq!(
            step.observe(&Observation::Present(found), tag),
            Ok(Action::Settled)
        );
        assert_eq!(
            step.state,
            StepState::Done {
                resource_id: id(1),
                adopted: true
            }
        );

        let step = &mut op.steps[1];
        step.begin_attempt(&policy).unwrap();
        let other = ObservedResource {
            id: id(2),
            spec: ResourceSpec::OidcRealm,
            tag: None,
        };
        step.observe(&Observation::Present(other), tag).unwrap();
        assert_eq!(
            step.state,
            StepState::Blocked {
                reason: BlockReason::Conflict
            }
        );
        assert_eq!(op.next(), Next::Finish(OperationOutcome::Partial));
    }

    #[test]
    fn retryable_failures_stay_uncertain_until_the_budget_is_spent() {
        let mut op = op();
        let policy = RetryPolicy {
            max_attempts: 3,
            ..RetryPolicy::default()
        };
        let step = &mut op.steps[0];
        for attempt in 1..=3 {
            assert_eq!(step.begin_attempt(&policy), Ok(Begin::Attempt(attempt)));
            step.failed(ErrorClass::Timeout).unwrap();
            assert_eq!(step.state, StepState::Uncertain);
        }
        assert_eq!(step.begin_attempt(&policy), Ok(Begin::Exhausted));
        assert_eq!(step.state, StepState::GaveUp);
        assert_eq!(op.next(), Next::Finish(OperationOutcome::Partial));
        op.finish(OperationOutcome::Partial).unwrap();
        assert_eq!(op.status(), "partial");

        op.retry().unwrap();
        assert_eq!(op.round, 1);
        assert_eq!(op.state, OperationState::Pending);
        assert_eq!(op.steps[0].state, StepState::Pending);
        assert_eq!(op.steps[0].attempts, 0);
        assert!(op.retry().is_err(), "only a partial operation retries");
    }

    #[test]
    fn rejected_is_final_and_illegal_transitions_are_refused() {
        let mut op = op();
        let step = &mut op.steps[0];
        assert!(
            step.observe(&Observation::Absent, OperationId(Uuid::nil()))
                .is_err()
        );
        assert!(step.created(id(1)).is_err());
        step.begin_attempt(&RetryPolicy::default()).unwrap();
        step.failed(ErrorClass::Rejected).unwrap();
        assert_eq!(
            step.state,
            StepState::Blocked {
                reason: BlockReason::Rejected
            }
        );
        assert!(step.begin_attempt(&RetryPolicy::default()).is_err());
    }

    #[test]
    fn a_running_operation_with_an_unrecorded_attempt_reads_uncertain() {
        let mut op = op();
        op.state = OperationState::Running;
        op.steps[0].begin_attempt(&RetryPolicy::default()).unwrap();
        assert_eq!(op.status(), "uncertain");
    }

    #[derive(Debug, Clone)]
    enum Event {
        Begin,
        ObserveAbsent,
        ObserveMatch,
        ObserveOther,
        Created,
        Failed(ErrorClass),
        Retry,
    }

    fn event() -> impl Strategy<Value = Event> {
        prop_oneof![
            4 => Just(Event::Begin),
            3 => Just(Event::ObserveAbsent),
            1 => Just(Event::ObserveMatch),
            1 => Just(Event::ObserveOther),
            3 => Just(Event::Created),
            2 => Just(Event::Failed(ErrorClass::Timeout)),
            1 => Just(Event::Failed(ErrorClass::Rejected)),
            1 => Just(Event::Retry),
        ]
    }

    proptest! {
        /// Whatever order a buggy or crashing caller drives it in, the machine never marks
        /// a step done without a provider id, never exceeds the round's budget, and never
        /// lets an operation succeed with a step that is not done.
        #[test]
        fn invariants_hold_for_any_event_sequence(events in prop::collection::vec(event(), 0..80)) {
            let policy = RetryPolicy { max_attempts: 3, ..RetryPolicy::default() };
            let mut op = op();
            let mut n = 0;
            for event in events {
                let tag = op.id;
                let next = op.next();
                match (event, next) {
                    (Event::Retry, _) => { let _ = op.retry(); }
                    (_, Next::Finish(outcome)) => { let _ = op.finish(outcome); }
                    (event, Next::Attempt(i)) => {
                        let step = &mut op.steps[i];
                        let spec = step.resource.spec.clone();
                        let _ = match event {
                            Event::Begin => step.begin_attempt(&policy).map(|_| ()),
                            Event::ObserveAbsent => step.observe(&Observation::Absent, tag).map(|_| ()),
                            Event::ObserveMatch => step.observe(&Observation::Present(ObservedResource { id: id(99), spec, tag: None }), tag).map(|_| ()),
                            Event::ObserveOther => step.observe(&Observation::Present(ObservedResource { id: id(98), spec: ResourceSpec::OidcRealm, tag: None }), tag).map(|_| ()),
                            Event::Created => { n += 1; step.created(id(n)) }
                            Event::Failed(class) => step.failed(class),
                            Event::Retry => Ok(()),
                        };
                    }
                }
                for step in &op.steps {
                    prop_assert!(step.attempts <= policy.max_attempts);
                    if let StepState::Done { resource_id, .. } = &step.state {
                        prop_assert!(!resource_id.as_str().is_empty());
                    }
                }
                if op.state == OperationState::Succeeded {
                    prop_assert!(op.steps.iter().all(|s| s.resource_id().is_some()));
                }
            }
        }
    }
}
