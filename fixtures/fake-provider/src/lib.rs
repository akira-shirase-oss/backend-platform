//! A fault-injecting fake provider — the substrate of the partial-provisioning drill and
//! the control-API load run.
//!
//! **It deliberately does not enforce unique names** (unless asked to): a create always
//! makes a new resource with a fresh id, the way many cloud APIs and identity-provider
//! client registrations behave. So the only thing standing between a blind retry and a
//! duplicate is the reconciler's observe-before-create rule — which is exactly what the
//! drills measure, counted on the provider side.
//!
//! Faults, chosen per call by scripted rules or a seeded random schedule:
//!
//! - [`Fault::FailBefore`]: answer "unavailable" with no effect.
//! - [`Fault::LoseReply`]: apply the create, then answer "unavailable" (the reply is lost).
//! - [`Fault::Hang`]: apply the create (if any), then never answer in time. In process the
//!   call returns `Timeout` at once; over HTTP the reply is held past the caller's deadline.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use platform_adapters::{Provider, ProviderError};
use platform_domain::{
    CreateRequest, Observation, ObservedResource, OperationId, ProviderName, ProviderResourceId,
    ResourceKind, ResourceSpec,
};
use serde::{Deserialize, Serialize};

pub mod server;

/// A fault injected into one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    /// Fail with no effect.
    FailBefore,
    /// Apply the effect, then fail (the reply is lost).
    LoseReply,
    /// Apply the effect, then do not answer in time.
    Hang,
}

/// Which call a rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    /// A lookup.
    Lookup,
    /// A create.
    Create,
}

/// Inject `fault` into the `nth` (1-based) call matching `on` and `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultRule {
    /// Lookup or create.
    pub on: CallKind,
    /// Only calls for this kind (any kind when absent).
    #[serde(default)]
    pub kind: Option<ResourceKind>,
    /// Which matching call.
    pub nth: u32,
    /// The fault.
    pub fault: Fault,
}

/// A seeded random fault schedule: each call faults with probability `per_million / 10⁶`,
/// at most `max` times in total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RandomFaults {
    /// The seed (same seed, same schedule).
    pub seed: u64,
    /// Fault probability per call, in millionths.
    pub per_million: u32,
    /// Most faults to inject; unlimited when absent.
    #[serde(default)]
    pub max: Option<u64>,
}

/// The fault configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultConfig {
    /// Scripted faults.
    #[serde(default)]
    pub rules: Vec<FaultRule>,
    /// A random schedule.
    #[serde(default)]
    pub random: Option<RandomFaults>,
}

/// A resource the fake holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FakeResource {
    /// Provider id.
    pub id: ProviderResourceId,
    /// Kind.
    pub kind: ResourceKind,
    /// Name.
    pub name: ProviderName,
    /// Spec.
    pub spec: ResourceSpec,
    /// Creating operation.
    pub tag: Option<OperationId>,
}

/// What a call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    /// Lookup found nothing.
    Absent,
    /// Lookup found the resource.
    Present,
    /// Create made a resource.
    Created,
    /// Create refused: name taken (unique-names mode only).
    AlreadyExists,
    /// Failed before any effect.
    Failed,
}

/// One recorded call (when call recording is on).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    /// Order of arrival.
    pub seq: u64,
    /// Lookup or create.
    pub call: CallKind,
    /// Kind.
    pub kind: ResourceKind,
    /// Name.
    pub name: ProviderName,
    /// The creating operation (creates only).
    pub tag: Option<OperationId>,
    /// The injected fault.
    pub fault: Option<Fault>,
    /// What happened to state.
    pub outcome: CallOutcome,
}

/// A name held more than once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Duplicate {
    /// Kind.
    pub kind: ResourceKind,
    /// Name.
    pub name: ProviderName,
    /// How many resources share it.
    pub count: usize,
}

/// Provider-side counts: the drill's and the load run's evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    /// Resources held.
    pub resources: usize,
    /// Distinct names held.
    pub names: usize,
    /// Names held more than once (must be empty).
    pub duplicates: Vec<Duplicate>,
    /// Lookups served.
    pub lookups: u64,
    /// Creates served.
    pub creates: u64,
    /// Faults injected.
    pub faults_injected: u64,
}

#[derive(Debug)]
struct State {
    resources: HashMap<(ResourceKind, ProviderName), Vec<FakeResource>>,
    next_id: u64,
    unique_names: bool,
    rules: Vec<(FaultRule, u32)>,
    random: Option<(u64, u32, Option<u64>)>,
    calls: Option<Vec<Call>>,
    seq: u64,
    lookups: u64,
    creates: u64,
    faults: u64,
}

/// `SplitMix64`: a tiny, well-mixed, seedable generator (no dependency, deterministic).
pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl State {
    fn pick_fault(&mut self, call: CallKind, kind: ResourceKind) -> Option<Fault> {
        for (rule, matched) in &mut self.rules {
            if rule.on == call && rule.kind.is_none_or(|k| k == kind) {
                *matched += 1;
                if *matched == rule.nth {
                    self.faults += 1;
                    return Some(rule.fault);
                }
            }
        }
        if let Some((state, per_million, remaining)) = &mut self.random
            && remaining.is_none_or(|r| r > 0)
        {
            let draw = splitmix64(state);
            if draw % 1_000_000 < u64::from(*per_million) {
                if let Some(r) = remaining {
                    *r -= 1;
                }
                self.faults += 1;
                let choice = splitmix64(state);
                return Some(match (call, choice % 3) {
                    (_, 0) => Fault::FailBefore,
                    (CallKind::Create, 1) => Fault::LoseReply,
                    _ => Fault::Hang,
                });
            }
        }
        None
    }

    fn record(
        &mut self,
        call: CallKind,
        kind: ResourceKind,
        name: &ProviderName,
        tag: Option<OperationId>,
        fault: Option<Fault>,
        outcome: CallOutcome,
    ) {
        self.seq += 1;
        let seq = self.seq;
        if let Some(calls) = &mut self.calls {
            calls.push(Call {
                seq,
                call,
                kind,
                name: name.clone(),
                tag,
                fault,
                outcome,
            });
        }
    }
}

/// What a call produced, before the transport decides how to deliver a hang.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply<T> {
    /// The answer the caller gets (if it is still waiting).
    pub result: Result<T, ProviderError>,
    /// True when the answer must not arrive before the caller's deadline.
    pub hang: bool,
}

/// The fake provider.
#[derive(Debug)]
pub struct FakeProvider {
    state: Mutex<State>,
}

/// How the fake starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeOptions {
    /// Refuse a create whose name exists (like PostgreSQL's `CREATE DATABASE`).
    pub unique_names: bool,
    /// Keep every call (drills); off for long load runs.
    pub record_calls: bool,
    /// The initial faults.
    pub faults: FaultConfig,
}

impl FakeProvider {
    /// A fresh, empty fake.
    pub fn new(options: FakeOptions) -> Self {
        let mut state = State {
            resources: HashMap::new(),
            next_id: 0,
            unique_names: options.unique_names,
            rules: Vec::new(),
            random: None,
            calls: options.record_calls.then(Vec::new),
            seq: 0,
            lookups: 0,
            creates: 0,
            faults: 0,
        };
        apply_faults(&mut state, options.faults);
        Self {
            state: Mutex::new(state),
        }
    }

    // The lock is never held across an await: every method finishes its work and drops it.
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock leaves plain data behind; keep serving it.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Replaces the fault configuration (counters of rules restart).
    pub fn set_faults(&self, faults: FaultConfig) {
        apply_faults(&mut self.lock(), faults);
    }

    /// Places a resource directly, as if an earlier, forgotten operation had made it.
    pub fn seed(
        &self,
        kind: ResourceKind,
        name: ProviderName,
        spec: ResourceSpec,
    ) -> ProviderResourceId {
        let mut state = self.lock();
        let id = next_id(&mut state, kind);
        state
            .resources
            .entry((kind, name.clone()))
            .or_default()
            .push(FakeResource {
                id: id.clone(),
                kind,
                name,
                spec,
                tag: None,
            });
        id
    }

    /// Serves a lookup.
    pub fn lookup_now(&self, kind: ResourceKind, name: &ProviderName) -> Reply<Observation> {
        let mut state = self.lock();
        state.lookups += 1;
        let fault = state.pick_fault(CallKind::Lookup, kind);
        if fault == Some(Fault::FailBefore) {
            state.record(
                CallKind::Lookup,
                kind,
                name,
                None,
                fault,
                CallOutcome::Failed,
            );
            return Reply {
                result: Err(ProviderError::Unavailable),
                hang: false,
            };
        }
        let found = state
            .resources
            .get(&(kind, name.clone()))
            .and_then(|list| list.first())
            .cloned();
        let outcome = if found.is_some() {
            CallOutcome::Present
        } else {
            CallOutcome::Absent
        };
        state.record(CallKind::Lookup, kind, name, None, fault, outcome);
        let observation = found.map_or(Observation::Absent, |r| {
            Observation::Present(ObservedResource {
                id: r.id,
                spec: r.spec,
                tag: r.tag,
            })
        });
        match fault {
            Some(Fault::Hang) => Reply {
                result: Err(ProviderError::Timeout),
                hang: true,
            },
            Some(Fault::LoseReply) => Reply {
                result: Err(ProviderError::Unavailable),
                hang: false,
            },
            _ => Reply {
                result: Ok(observation),
                hang: false,
            },
        }
    }

    /// Serves a create.
    pub fn create_now(&self, request: &CreateRequest) -> Reply<ProviderResourceId> {
        let mut state = self.lock();
        state.creates += 1;
        let fault = state.pick_fault(CallKind::Create, request.kind);
        let key = (request.kind, request.name.clone());
        if fault == Some(Fault::FailBefore) {
            state.record(
                CallKind::Create,
                request.kind,
                &request.name,
                Some(request.tag),
                fault,
                CallOutcome::Failed,
            );
            return Reply {
                result: Err(ProviderError::Unavailable),
                hang: false,
            };
        }
        if state.unique_names && state.resources.get(&key).is_some_and(|l| !l.is_empty()) {
            state.record(
                CallKind::Create,
                request.kind,
                &request.name,
                Some(request.tag),
                fault,
                CallOutcome::AlreadyExists,
            );
            return Reply {
                result: Err(ProviderError::AlreadyExists),
                hang: fault == Some(Fault::Hang),
            };
        }
        let id = next_id(&mut state, request.kind);
        state.resources.entry(key).or_default().push(FakeResource {
            id: id.clone(),
            kind: request.kind,
            name: request.name.clone(),
            spec: request.spec.clone(),
            tag: Some(request.tag),
        });
        state.record(
            CallKind::Create,
            request.kind,
            &request.name,
            Some(request.tag),
            fault,
            CallOutcome::Created,
        );
        match fault {
            Some(Fault::LoseReply) => Reply {
                result: Err(ProviderError::Unavailable),
                hang: false,
            },
            Some(Fault::Hang) => Reply {
                result: Err(ProviderError::Timeout),
                hang: true,
            },
            _ => Reply {
                result: Ok(id),
                hang: false,
            },
        }
    }

    /// Every resource held, sorted by kind, name and id.
    pub fn resources(&self) -> Vec<FakeResource> {
        let state = self.lock();
        let mut all: Vec<_> = state.resources.values().flatten().cloned().collect();
        all.sort_by(|a, b| (a.kind, &a.name, &a.id).cmp(&(b.kind, &b.name, &b.id)));
        all
    }

    /// Every recorded call, in order (empty unless recording is on).
    pub fn calls(&self) -> Vec<Call> {
        self.lock().calls.clone().unwrap_or_default()
    }

    /// Provider-side counts, duplicates included.
    pub fn inventory(&self) -> Inventory {
        let state = self.lock();
        let mut duplicates: Vec<_> = state
            .resources
            .iter()
            .filter(|(_, list)| list.len() > 1)
            .map(|((kind, name), list)| Duplicate {
                kind: *kind,
                name: name.clone(),
                count: list.len(),
            })
            .collect();
        duplicates.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
        Inventory {
            resources: state.resources.values().map(Vec::len).sum(),
            names: state.resources.values().filter(|l| !l.is_empty()).count(),
            duplicates,
            lookups: state.lookups,
            creates: state.creates,
            faults_injected: state.faults,
        }
    }
}

fn apply_faults(state: &mut State, faults: FaultConfig) {
    state.rules = faults.rules.into_iter().map(|r| (r, 0)).collect();
    state.random = faults
        .random
        .map(|r| (r.seed, r.per_million.min(1_000_000), r.max));
}

fn next_id(state: &mut State, kind: ResourceKind) -> ProviderResourceId {
    state.next_id += 1;
    let prefix = match kind {
        ResourceKind::DatabaseRole => "role",
        ResourceKind::Database => "db",
        ResourceKind::Bucket => "bkt",
        ResourceKind::OidcRealm => "realm",
        ResourceKind::OidcClient => "client",
    };
    #[expect(
        clippy::expect_used,
        reason = "the id format is a constant valid shape"
    )]
    ProviderResourceId::parse(&format!("{prefix}-{:08}", state.next_id))
        .expect("invariant: generated ids are printable ASCII and short")
}

/// The in-process form: a hang returns `Timeout` at once (the caller's deadline is
/// simulated); the effect has already been applied.
impl Provider for FakeProvider {
    fn lookup(
        &self,
        kind: ResourceKind,
        name: &ProviderName,
    ) -> impl Future<Output = Result<Observation, ProviderError>> + Send {
        std::future::ready(self.lookup_now(kind, name).result)
    }

    fn create(
        &self,
        request: &CreateRequest,
    ) -> impl Future<Output = Result<ProviderResourceId, ProviderError>> + Send {
        std::future::ready(self.create_now(request).result)
    }
}

/// How long the HTTP form holds a hanging reply (far past any caller deadline).
pub const DEFAULT_HANG: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use platform_domain::{LogicalName, ProjectId, provider_name};
    use uuid::Uuid;

    use super::*;

    fn request() -> CreateRequest {
        let p = ProjectId::parse("shop").unwrap();
        let n = LogicalName::parse("files").unwrap();
        CreateRequest {
            kind: ResourceKind::Bucket,
            name: provider_name(&p, ResourceKind::Bucket, &n),
            spec: ResourceSpec::Bucket { quota_bytes: 1 },
            tag: OperationId(Uuid::nil()),
        }
    }

    #[test]
    fn a_blind_repeat_create_makes_a_duplicate_and_the_inventory_shows_it() {
        let fake = FakeProvider::new(FakeOptions::default());
        let r = request();
        assert!(fake.create_now(&r).result.is_ok());
        assert!(fake.create_now(&r).result.is_ok());
        let inventory = fake.inventory();
        assert_eq!((inventory.resources, inventory.names), (2, 1));
        assert_eq!(inventory.duplicates[0].count, 2);
    }

    #[test]
    fn unique_mode_refuses_a_taken_name() {
        let fake = FakeProvider::new(FakeOptions {
            unique_names: true,
            ..FakeOptions::default()
        });
        let r = request();
        assert!(fake.create_now(&r).result.is_ok());
        assert_eq!(
            fake.create_now(&r).result,
            Err(ProviderError::AlreadyExists)
        );
        assert_eq!(fake.inventory().duplicates.len(), 0);
    }

    #[test]
    fn scripted_faults_hit_the_nth_matching_call() {
        let faults = FaultConfig {
            rules: vec![
                FaultRule {
                    on: CallKind::Create,
                    kind: Some(ResourceKind::Bucket),
                    nth: 1,
                    fault: Fault::LoseReply,
                },
                FaultRule {
                    on: CallKind::Lookup,
                    kind: None,
                    nth: 2,
                    fault: Fault::FailBefore,
                },
            ],
            random: None,
        };
        let fake = FakeProvider::new(FakeOptions {
            record_calls: true,
            faults,
            ..FakeOptions::default()
        });
        let r = request();
        assert_eq!(
            fake.lookup_now(r.kind, &r.name).result,
            Ok(Observation::Absent)
        );
        // The create takes effect but its reply is lost.
        assert_eq!(fake.create_now(&r).result, Err(ProviderError::Unavailable));
        assert_eq!(
            fake.lookup_now(r.kind, &r.name).result,
            Err(ProviderError::Unavailable)
        );
        assert!(matches!(
            fake.lookup_now(r.kind, &r.name).result,
            Ok(Observation::Present(_))
        ));
        let calls = fake.calls();
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[1].outcome, CallOutcome::Created);
        assert_eq!(calls[1].fault, Some(Fault::LoseReply));
        assert_eq!(fake.inventory().faults_injected, 2);
    }

    #[test]
    fn random_schedules_are_reproducible_and_capped() {
        let run = |seed| {
            let faults = FaultConfig {
                rules: vec![],
                random: Some(RandomFaults {
                    seed,
                    per_million: 500_000,
                    max: Some(5),
                }),
            };
            let fake = FakeProvider::new(FakeOptions {
                faults,
                record_calls: true,
                ..FakeOptions::default()
            });
            let r = request();
            for _ in 0..50 {
                let _ = fake.lookup_now(r.kind, &r.name);
            }
            (
                fake.calls().iter().map(|c| c.fault).collect::<Vec<_>>(),
                fake.inventory().faults_injected,
            )
        };
        assert_eq!(run(7), run(7));
        assert_eq!(run(7).1, 5);
    }
}
