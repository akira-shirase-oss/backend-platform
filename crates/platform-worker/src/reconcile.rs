//! One claimed operation, step by step.
//!
//! The order of effects is the whole design:
//!
//! 1. **Begin** the attempt and *commit it* (with the previous step's result) before any
//!    provider call — intent first, so a crash leaves the step visibly `uncertain` and the
//!    crash still counts against the attempt budget.
//! 2. **Observe** the deterministic name. Present with the same spec → adopt; different
//!    spec → blocked; absent → create.
//! 3. **Create** under the deterministic name, tagged with the operation id.
//! 4. The result is recorded together with the next step's begin (one statement), or with
//!    the finish. If the worker dies first, the next worker observes and adopts.
//!
//! Every provider call has a deadline; every failure leaves the step uncertain (except a
//! rejection); every attempt starts with an observation. There is no code path that
//! creates without observing in the same attempt.

use std::future::Future;
use std::time::Duration;

use platform_adapters::{Provider, ProviderError};
use platform_db::Claimed;
use platform_domain::{
    Action, Begin, ErrorClass, Next, Observation, OperationOutcome, OperationState, RetryPolicy,
    Step, TransitionError,
};
use thiserror::Error;

use crate::log::{LogError, OperationLog};

/// Named points in an attempt where a drill may stop the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Point {
    /// The attempt is committed; no provider has been called.
    AfterBegin,
    /// The lookup returned; nothing else happened.
    AfterLookup,
    /// The lease check for the create passed; the create is not yet sent (the residual
    /// window of D-029).
    BeforeCreate,
    /// The create returned (its effect applied); the result is not recorded.
    AfterCreate,
    /// The round's finish is committed.
    AfterFinish,
}

impl Point {
    /// Every point.
    pub const ALL: [Self; 5] = [
        Self::AfterBegin,
        Self::AfterLookup,
        Self::BeforeCreate,
        Self::AfterCreate,
        Self::AfterFinish,
    ];

    /// The spelling used by `BP_HALT_AT`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AfterBegin => "after-begin",
            Self::AfterLookup => "after-lookup",
            Self::BeforeCreate => "before-create",
            Self::AfterCreate => "after-create",
            Self::AfterFinish => "after-finish",
        }
    }

    /// Parses the `BP_HALT_AT` spelling.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == value)
    }
}

/// The worker stopped at a fault point (a drill's simulated or real crash).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("halted at a fault point")]
pub struct Halted;

/// Fault points. Production uses [`NoHooks`].
pub trait Hooks: Send + Sync {
    /// Called at each point; `Err(Halted)` stops the worker there.
    fn at(
        &self,
        point: Point,
        position: u16,
        attempt: u16,
    ) -> impl Future<Output = Result<(), Halted>> + Send;
}

/// No fault points.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHooks;

impl Hooks for NoHooks {
    fn at(
        &self,
        _point: Point,
        _position: u16,
        _attempt: u16,
    ) -> impl Future<Output = Result<(), Halted>> + Send {
        std::future::ready(Ok(()))
    }
}

/// Deadlines and budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileConfig {
    /// Attempts per step per round, and backoff.
    pub policy: RetryPolicy,
    /// Deadline of each provider call.
    pub call_timeout: Duration,
    /// Lease length; renewed by every save.
    pub lease: Duration,
}

impl ReconcileConfig {
    /// An attempt makes at most three provider calls (lookup, create, re-lookup) and then
    /// backs off; all of it must fit well inside one lease, or a healthy worker could lose
    /// its lease mid-attempt.
    pub fn validate(&self) -> Result<(), String> {
        let worst = self
            .call_timeout
            .saturating_mul(3)
            .saturating_add(self.policy.cap);
        if worst.saturating_mul(2) > self.lease {
            return Err(format!(
                "lease {:?} must be at least twice 3 × call timeout + backoff cap ({worst:?})",
                self.lease
            ));
        }
        if self.policy.max_attempts == 0 {
            return Err("max attempts must be at least 1".to_owned());
        }
        Ok(())
    }
}

/// Why reconciling stopped before the round ended.
#[derive(Debug, Error)]
pub enum RunError {
    /// A fault point stopped the worker.
    #[error(transparent)]
    Halted(#[from] Halted),
    /// The log refused a write (lease lost) or failed.
    #[error(transparent)]
    Log(#[from] LogError),
    /// A bug: the state machine refused a transition.
    #[error("the state machine refused a transition")]
    Transition(#[from] TransitionError),
    /// The local lease deadline passed before a provider call (the worker was paused or
    /// starved): it stops instead of calling, because another worker may own the
    /// operation now (F4).
    #[error("the lease ran out before a provider call; another worker may own the operation")]
    LeaseExpired,
}

/// The lease as this worker knows it, on its own monotonic clock: measured from just
/// before the last save that renewed it, so it can only be earlier than the database's.
#[derive(Debug, Clone, Copy)]
struct LocalLease {
    valid_until: tokio::time::Instant,
}

impl LocalLease {
    /// Refuses a provider call that could not finish (its own deadline included) before
    /// the lease runs out.
    fn check(&self, call_timeout: Duration) -> Result<(), RunError> {
        if tokio::time::Instant::now() + call_timeout > self.valid_until {
            Err(RunError::LeaseExpired)
        } else {
            Ok(())
        }
    }
}

async fn call<T>(
    deadline: Duration,
    future: impl Future<Output = Result<T, ProviderError>>,
) -> Result<T, ErrorClass> {
    match tokio::time::timeout(deadline, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error.class()),
        Err(_) => Err(ErrorClass::Timeout),
    }
}

/// One attempt of one step (already begun and committed). Leaves the step done, blocked or
/// uncertain; never writes to the log itself.
async fn attempt<P: Provider, H: Hooks>(
    step: &mut Step,
    operation: platform_domain::OperationId,
    provider: &P,
    hooks: &H,
    config: &ReconcileConfig,
    lease: LocalLease,
) -> Result<(), RunError> {
    let (position, attempt) = (step.position, step.attempts);
    let kind = step.resource.kind;
    let name = step.resource.name.clone();
    lease.check(config.call_timeout)?;
    let observed = call(config.call_timeout, provider.lookup(kind, &name)).await;
    hooks.at(Point::AfterLookup, position, attempt).await?;
    let observation = match observed {
        Ok(observation) => observation,
        Err(class) => return Ok(step.failed(class)?),
    };
    let Action::Create(request) = step.observe(&observation, operation)? else {
        return Ok(());
    };
    lease.check(config.call_timeout)?;
    hooks.at(Point::BeforeCreate, position, attempt).await?;
    let created = call(config.call_timeout, provider.create(&request)).await;
    hooks.at(Point::AfterCreate, position, attempt).await?;
    match created {
        Ok(id) => step.created(id)?,
        Err(ErrorClass::AlreadyExists) => {
            // Someone holds the name (a lost earlier reply): observe again and adopt.
            lease.check(config.call_timeout)?;
            match call(config.call_timeout, provider.lookup(kind, &name)).await {
                // One holder: adopt it (or block on a spec conflict). Several: blocked as a
                // duplicate, never adopted.
                Ok(again @ (Observation::Present(_) | Observation::Many { .. })) => {
                    let _settled = step.observe(&again, operation)?;
                }
                Ok(Observation::Absent) => step.failed(ErrorClass::AlreadyExists)?,
                Err(class) => step.failed(class)?,
            }
        }
        Err(class) => step.failed(class)?,
    }
    Ok(())
}

/// Drives a claimed operation to the end of its round, or until the lease is lost or a
/// fault point halts it. `entropy` feeds the backoff jitter.
pub async fn run_claimed<L: OperationLog, P: Provider, H: Hooks>(
    log: &L,
    provider: &P,
    hooks: &H,
    config: &ReconcileConfig,
    claimed: Claimed,
    entropy: &mut u64,
) -> Result<OperationOutcome, RunError> {
    let Claimed {
        lease,
        mut operation,
    } = claimed;
    // A step whose new state is not yet committed; it goes out with the next write.
    let mut unsaved: Option<usize> = None;
    // No provider call happens before the first save renews the lease.
    let mut local = LocalLease {
        valid_until: tokio::time::Instant::now(),
    };
    loop {
        match operation.next() {
            Next::Finish(outcome) => {
                operation.finish(outcome)?;
                let steps: Vec<&Step> = unsaved.map(|i| &operation.steps[i]).into_iter().collect();
                let state = match outcome {
                    OperationOutcome::Succeeded => OperationState::Succeeded,
                    OperationOutcome::Partial => OperationState::Partial,
                };
                log.finish(&lease, state, &steps).await?;
                hooks.at(Point::AfterFinish, 0, 0).await?;
                return Ok(outcome);
            }
            Next::Attempt(i) => {
                let step = &mut operation.steps[i];
                if step.last_error.is_some() && step.attempts > 0 {
                    let pause = config.policy.backoff(step.attempts, next_entropy(entropy));
                    if !pause.is_zero() {
                        tokio::time::sleep(pause).await;
                    }
                }
                let begin = step.begin_attempt(&config.policy)?;
                let mut batch: Vec<&Step> = Vec::with_capacity(2);
                if let Some(previous) = unsaved.take()
                    && previous != i
                {
                    batch.push(&operation.steps[previous]);
                }
                batch.push(&operation.steps[i]);
                let sent = tokio::time::Instant::now();
                log.save(&lease, &batch, config.lease).await?;
                local.valid_until = sent + config.lease;
                let Begin::Attempt(number) = begin else {
                    continue; // gave up; saved; the next turn finishes the round as partial
                };
                let step = &mut operation.steps[i];
                hooks.at(Point::AfterBegin, step.position, number).await?;
                attempt(step, operation.id, provider, hooks, config, local).await?;
                unsaved = Some(i);
            }
        }
    }
}

/// A `SplitMix64` step: cheap, well-mixed jitter without a dependency.
pub fn next_entropy(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_validation_keeps_attempts_inside_the_lease() {
        let ok = ReconcileConfig {
            policy: RetryPolicy::default(),
            call_timeout: Duration::from_secs(2),
            lease: Duration::from_secs(20),
        };
        assert!(ok.validate().is_ok());
        let tight = ReconcileConfig {
            lease: Duration::from_secs(5),
            ..ok
        };
        assert!(tight.validate().is_err());
    }

    #[test]
    fn points_round_trip() {
        for point in Point::ALL {
            assert_eq!(Point::parse(point.as_str()), Some(point));
        }
    }
}
