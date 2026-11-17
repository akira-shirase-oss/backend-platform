//! Shared drill support: an in-memory operation log with the same fencing rules as the
//! PostgreSQL one, and the provider-side oracle every drill applies.
#![allow(dead_code, reason = "each drill binary uses a different subset")]
#![allow(
    clippy::unused_async_trait_impl,
    reason = "the in-memory log mirrors the async port; it never needs to await"
)]
#![allow(
    clippy::unwrap_used,
    reason = "test support: a panic is the failure report"
)]

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use platform_db::{Claimed, Lease};
use platform_domain::{
    Operation, OperationId, OperationState, Plan, ResourceKind, Step, StepState, TransitionError,
};
use platform_fake_provider::{CallKind, CallOutcome, FakeProvider};
use platform_worker::{LogError, OperationLog};
use uuid::Uuid;

#[derive(Debug)]
struct Stored {
    operation: Operation,
    owner: Option<Uuid>,
    token: i64,
    live: bool,
}

/// An operation log in memory. What a crashed worker had not saved is simply gone, exactly
/// as with the database.
#[derive(Debug, Default)]
pub(crate) struct MemLog {
    ops: Mutex<Vec<Stored>>,
}

impl MemLog {
    pub(crate) fn insert(&self, operation: Operation) {
        self.ops.lock().unwrap().push(Stored {
            operation,
            owner: None,
            token: 0,
            live: false,
        });
    }

    /// Time passes after a crash: every lease runs out.
    pub(crate) fn expire_leases(&self) {
        for stored in self.ops.lock().unwrap().iter_mut() {
            stored.live = false;
        }
    }

    pub(crate) fn get(&self, id: OperationId) -> Operation {
        let ops = self.ops.lock().unwrap();
        ops.iter()
            .find(|s| s.operation.id == id)
            .unwrap()
            .operation
            .clone()
    }

    /// The operator's retry of a partial operation.
    pub(crate) fn retry(&self, id: OperationId) -> Result<(), TransitionError> {
        let mut ops = self.ops.lock().unwrap();
        let stored = ops.iter_mut().find(|s| s.operation.id == id).unwrap();
        stored.operation.retry()
    }

    fn write(
        &self,
        lease: &Lease,
        steps: &[&Step],
        finish: Option<OperationState>,
    ) -> Result<(), LogError> {
        let mut ops = self.ops.lock().unwrap();
        let stored = ops
            .iter_mut()
            .find(|s| s.operation.id == lease.operation)
            .ok_or(LogError::Fenced)?;
        let holds = stored.owner == Some(lease.worker)
            && stored.token == lease.token
            && stored.live
            && stored.operation.state == OperationState::Running;
        if !holds {
            return Err(LogError::Fenced);
        }
        for step in steps {
            stored.operation.steps[usize::from(step.position)] = (*step).clone();
        }
        if let Some(state) = finish {
            stored.operation.state = state;
            stored.owner = None;
            stored.live = false;
        }
        Ok(())
    }
}

impl OperationLog for MemLog {
    async fn claim(&self, worker: Uuid, _lease_for: Duration) -> Result<Option<Claimed>, LogError> {
        let mut ops = self.ops.lock().unwrap();
        let mut blocked_projects = std::collections::BTreeSet::new();
        for stored in ops.iter_mut() {
            let unfinished = !stored.operation.state.is_finished();
            let project = stored.operation.project.clone();
            if unfinished && !stored.live && !blocked_projects.contains(&project) {
                stored.operation.state = OperationState::Running;
                stored.owner = Some(worker);
                stored.token += 1;
                stored.live = true;
                let lease = Lease {
                    operation: stored.operation.id,
                    worker,
                    token: stored.token,
                };
                return Ok(Some(Claimed {
                    lease,
                    operation: stored.operation.clone(),
                }));
            }
            if unfinished {
                blocked_projects.insert(project);
            }
        }
        Ok(None)
    }

    async fn save(
        &self,
        lease: &Lease,
        steps: &[&Step],
        _lease_for: Duration,
    ) -> Result<(), LogError> {
        self.write(lease, steps, None)
    }

    async fn finish(
        &self,
        lease: &Lease,
        state: OperationState,
        steps: &[&Step],
    ) -> Result<(), LogError> {
        self.write(lease, steps, Some(state))
    }
}

/// The drill's pass condition, read on the provider side: exactly one resource per planned
/// resource, nothing else, every recorded id real — and when the operation succeeded,
/// every step recorded. With `calls` recorded, also: every create was immediately preceded
/// (for its name) by a delivered lookup that saw the name absent.
pub(crate) fn check(
    plan: &Plan,
    provider: &FakeProvider,
    operation: Option<&Operation>,
) -> Result<(), String> {
    let inventory = provider.inventory();
    if !inventory.duplicates.is_empty() {
        return Err(format!("duplicates: {:?}", inventory.duplicates));
    }
    let held: BTreeMap<(ResourceKind, String), String> = provider
        .resources()
        .into_iter()
        .map(|r| ((r.kind, r.name.to_string()), r.id.to_string()))
        .collect();
    for resource in &plan.resources {
        if !held.contains_key(&(resource.kind, resource.name.to_string())) {
            return Err(format!("missing {} {}", resource.kind, resource.name));
        }
    }
    if held.len() != plan.resources.len() {
        return Err(format!(
            "orphans: {} held, {} planned",
            held.len(),
            plan.resources.len()
        ));
    }
    if let Some(operation) = operation {
        for step in &operation.steps {
            if let StepState::Done { resource_id, .. } = &step.state {
                let key = (step.resource.kind, step.resource.name.to_string());
                if held.get(&key) != Some(&resource_id.to_string()) {
                    return Err(format!(
                        "step {} recorded {resource_id}, provider holds {:?}",
                        step.position,
                        held.get(&key)
                    ));
                }
            }
        }
        if operation.state == OperationState::Succeeded
            && operation.steps.iter().any(|s| s.resource_id().is_none())
        {
            return Err("succeeded with a step not done".to_owned());
        }
    }
    let calls = provider.calls();
    let mut last: BTreeMap<(ResourceKind, String), (CallKind, CallOutcome, bool)> = BTreeMap::new();
    for call in &calls {
        let key = (call.kind, call.name.to_string());
        if call.call == CallKind::Create {
            let observed_absent = matches!(
                last.get(&key),
                Some((CallKind::Lookup, CallOutcome::Absent, true))
            );
            if !observed_absent {
                return Err(format!(
                    "blind create of {} {} (call {})",
                    call.kind, call.name, call.seq
                ));
            }
        }
        last.insert(key, (call.call, call.outcome, call.fault.is_none()));
    }
    Ok(())
}
