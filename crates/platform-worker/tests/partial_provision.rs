//! Drill `partial-provision`, in process (`END_GOAL` failure case 1, against the fake).
//!
//! 10,000 randomized fault schedules. Each draws a plan (1–9 steps of every kind), a
//! provider fault schedule (failures before any effect, creates whose reply is lost,
//! calls that hang past the deadline after taking effect), simulated worker crashes at every
//! fault point (after the attempt is committed, after the lookup, after the create took
//! effect but before it was recorded, after the finish), and sometimes resources that an
//! earlier, forgotten operation already made. The operation is driven by the production
//! reconcile code (`run_claimed`) until it succeeds; a crashed worker's lease expires and
//! another resumes; a `partial` round is checked for visibility and then retried, as an
//! operator would.
//!
//! Pass: exactly one provider resource per planned resource, nothing orphaned, every
//! recorded id real, no create without a delivered "absent" observation — for every
//! schedule. `BP_DRILL_REPORT_DIR=<dir>` writes each mode's totals as JSON.
#![allow(clippy::unwrap_used, reason = "drill: a panic is the failure report")]
#![allow(
    clippy::unused_async_trait_impl,
    reason = "the crash hook mirrors the async port; it never needs to await"
)]

mod common;

use std::sync::Mutex;
use std::time::Duration;

use common::{MemLog, check};
use platform_adapters::Provider;
use platform_domain::{
    CreateRequest, DesiredState, IdentitySpec, LogicalName, Operation, OperationId,
    OperationOutcome, OperationState, Plan, ProjectId, ResourceKind, ResourceSpec, RetryPolicy,
    StepState,
};
use platform_fake_provider::{
    CallKind, CallOutcome, FakeOptions, FakeProvider, Fault, FaultConfig, FaultRule, RandomFaults,
    splitmix64,
};
use platform_worker::{Halted, Hooks, Point, ReconcileConfig, RunError, run_claimed};
use uuid::Uuid;

const SCHEDULES: u64 = 10_000;

/// Simulated crashes: each fault point halts with probability `per_million`, at most
/// `remaining` times.
struct Crashes {
    state: Mutex<(u64, u64, u32)>,
}

impl Crashes {
    fn new(seed: u64, per_million: u64, max: u32) -> Self {
        Self {
            state: Mutex::new((seed, per_million, max)),
        }
    }
}

impl Hooks for Crashes {
    async fn at(&self, _point: Point, _position: u16, _attempt: u16) -> Result<(), Halted> {
        let mut state = self.state.lock().unwrap();
        let (rng, per_million, remaining) = &mut *state;
        if *remaining > 0 && splitmix64(rng) % 1_000_000 < *per_million {
            *remaining -= 1;
            return Err(Halted);
        }
        Ok(())
    }
}

fn names(prefix: &str, n: u64) -> Vec<LogicalName> {
    (0..n)
        .map(|i| LogicalName::parse(&format!("{prefix}{i}")).unwrap())
        .collect()
}

fn random_plan(rng: &mut u64, project: &ProjectId) -> (DesiredState, Plan) {
    let desired = DesiredState {
        databases: names("db", 1 + splitmix64(rng) % 2),
        buckets: names("files", splitmix64(rng) % 3),
        identity: splitmix64(rng).is_multiple_of(2).then(|| IdentitySpec {
            clients: names("app", splitmix64(rng) % 3),
        }),
    };
    let plan = Plan::build(project, &desired).unwrap();
    (desired, plan)
}

fn config() -> ReconcileConfig {
    ReconcileConfig {
        // A small budget so schedules also exercise give-up → partial → operator retry.
        policy: RetryPolicy {
            max_attempts: 3,
            base: Duration::ZERO,
            cap: Duration::ZERO,
        },
        call_timeout: Duration::from_secs(1),
        lease: Duration::from_secs(10),
    }
}

/// What the provider promises about names (the drill runs each, F5/F6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    /// Neither unique names nor idempotency keys: a create that lands late can make a
    /// duplicate no reconciler can prevent; the pass condition is that every such
    /// duplicate is reported and none is ever hidden behind a success.
    Plain,
    /// Creates are de-duplicated by idempotency key: zero duplicates, always.
    Keys,
    /// A taken name is refused (`CREATE DATABASE`, bucket names): zero duplicates, always,
    /// through the `AlreadyExists` → look up → adopt path.
    Unique,
}

#[derive(Debug, Default, serde::Serialize)]
struct Totals {
    mode: Option<Mode>,
    schedules: u64,
    planned_resources: u64,
    provider_faults_injected: u64,
    late_effects_injected: u64,
    worker_crashes: u64,
    partial_rounds_retried: u64,
    pre_existing_resources_adopted: u64,
    resources_adopted_after_lost_reply_or_crash: u64,
    already_exists_answers: u64,
    idempotent_replays: u64,
    creates_sent: u64,
    lookups_sent: u64,
    duplicates_reported_visibly: u64,
    silent_duplicates: u64,
    orphans: u64,
    failures: u64,
}

/// A partial round that ended on duplicates (plain mode only): every duplicated name is
/// a step blocked `duplicate`, every such step names a real duplicate that a late create
/// caused, and nothing outside the plan exists.
fn check_reported_duplicates(
    plan: &Plan,
    provider: &FakeProvider,
    operation: &Operation,
) -> Result<u64, String> {
    let inventory = provider.inventory();
    let mut duplicated: Vec<String> = inventory
        .duplicates
        .iter()
        .map(|d| format!("{}:{}", d.kind, d.name))
        .collect();
    let mut blocked: Vec<String> = operation
        .steps
        .iter()
        .filter(|s| {
            s.state
                == StepState::Blocked {
                    reason: platform_domain::BlockReason::Duplicate,
                }
        })
        .map(|s| format!("{}:{}", s.resource.kind, s.resource.name))
        .collect();
    duplicated.sort();
    blocked.sort();
    if duplicated != blocked {
        return Err(format!("duplicates {duplicated:?} but blocked {blocked:?}"));
    }
    let late: Vec<String> = provider
        .calls()
        .iter()
        .filter(|c| c.fault == Some(Fault::LateEffect))
        .map(|c| format!("{}:{}", c.kind, c.name))
        .collect();
    if let Some(unexplained) = duplicated.iter().find(|d| !late.contains(d)) {
        return Err(format!("duplicate {unexplained} without a late create"));
    }
    let planned: Vec<String> = plan
        .resources
        .iter()
        .map(|r| format!("{}:{}", r.kind, r.name))
        .collect();
    if let Some(stray) = provider
        .resources()
        .iter()
        .map(|r| format!("{}:{}", r.kind, r.name))
        .find(|n| !planned.contains(n))
    {
        return Err(format!("orphans: {stray} is not planned"));
    }
    Ok(duplicated.len() as u64)
}

async fn run_schedule(seed: u64, mode: Mode, totals: &mut Totals) -> Result<(), String> {
    let mut rng = seed.wrapping_mul(0xA24B_AED4_963E_E407) ^ 0x0005_DEEC_E66D;
    let project = ProjectId::parse("drill").unwrap();
    let (_desired, plan) = random_plan(&mut rng, &project);
    let faults = FaultConfig {
        rules: Vec::new(),
        random: Some(RandomFaults {
            seed: splitmix64(&mut rng),
            per_million: 100_000 + u32::try_from(splitmix64(&mut rng) % 400_000).unwrap(),
            max: Some(splitmix64(&mut rng) % 26),
        }),
    };
    let provider = FakeProvider::new(FakeOptions {
        record_calls: true,
        faults,
        unique_names: mode == Mode::Unique,
        honor_idempotency_keys: mode == Mode::Keys,
        ..FakeOptions::default()
    });
    // Sometimes an earlier, forgotten operation already made one of the resources.
    let mut seeded = 0;
    if splitmix64(&mut rng) % 10 < 3 {
        let pick = &plan.resources
            [usize::try_from(splitmix64(&mut rng) % plan.resources.len() as u64).unwrap()];
        provider.seed(pick.kind, pick.name.clone(), pick.spec.clone());
        seeded = 1;
    }
    let crashes = Crashes::new(
        splitmix64(&mut rng),
        50_000 + splitmix64(&mut rng) % 150_000,
        u32::try_from(splitmix64(&mut rng) % 7).unwrap(),
    );
    let log = MemLog::default();
    let id = OperationId(Uuid::from_u64_pair(seed, 1));
    log.insert(Operation::new(id, 1, &plan));
    let config = config();
    let worker = Uuid::from_u64_pair(seed, 2);
    let mut entropy = seed;
    let mut runs = 0;
    loop {
        runs += 1;
        if runs > 1_000 {
            return Err(format!("seed {seed}: did not converge"));
        }
        if let Some(claimed) = platform_worker::OperationLog::claim(&log, worker, config.lease)
            .await
            .unwrap()
        {
            match run_claimed(&log, &provider, &crashes, &config, claimed, &mut entropy).await {
                Ok(_) => {}
                Err(RunError::Halted(_)) => {
                    totals.worker_crashes += 1;
                    log.expire_leases();
                }
                Err(error) => return Err(format!("seed {seed}: {error}")),
            }
        }
        let operation = log.get(id);
        match operation.state {
            OperationState::Succeeded => {
                // Success means exactly the plan: never an extra resource.
                if !provider.inventory().duplicates.is_empty() {
                    totals.silent_duplicates += 1;
                }
                check(&plan, &provider, Some(&operation))
                    .map_err(|e| format!("seed {seed}: {e}"))?;
                break;
            }
            OperationState::Partial
                if operation.steps.iter().any(|s| {
                    s.state
                        == StepState::Blocked {
                            reason: platform_domain::BlockReason::Duplicate,
                        }
                }) =>
            {
                if mode != Mode::Plain {
                    return Err(format!("seed {seed}: duplicates in {mode:?} mode"));
                }
                totals.duplicates_reported_visibly +=
                    check_reported_duplicates(&plan, &provider, &operation)
                        .map_err(|e| format!("seed {seed}: {e}"))?;
                break;
            }
            OperationState::Partial => {
                check_visible_partial(&provider, &operation)
                    .map_err(|e| format!("seed {seed}: {e}"))?;
                totals.partial_rounds_retried += 1;
                log.retry(id).map_err(|e| format!("seed {seed}: {e}"))?;
            }
            OperationState::Pending | OperationState::Running => {}
        }
    }
    record(&plan, &provider, &log.get(id), seeded, totals);
    Ok(())
}

/// A visible partial round: every step that recorded an id holds exactly one provider
/// resource, the one it recorded.
fn check_visible_partial(provider: &FakeProvider, operation: &Operation) -> Result<(), String> {
    for step in operation.steps.iter().filter(|s| s.resource_id().is_some()) {
        let held = provider
            .resources()
            .into_iter()
            .filter(|r| r.name == step.resource.name && r.kind == step.resource.kind)
            .count();
        if held != 1 {
            return Err(format!(
                "partial round recorded step {} with {held} provider resources",
                step.position
            ));
        }
    }
    Ok(())
}

/// Adds one finished schedule to the totals.
fn record(
    plan: &Plan,
    provider: &FakeProvider,
    operation: &Operation,
    seeded: u64,
    totals: &mut Totals,
) {
    let inventory = provider.inventory();
    let calls = provider.calls();
    totals.schedules += 1;
    totals.planned_resources += plan.resources.len() as u64;
    totals.provider_faults_injected += inventory.faults_injected;
    totals.late_effects_injected += calls
        .iter()
        .filter(|c| c.fault == Some(Fault::LateEffect))
        .count() as u64;
    totals.already_exists_answers += calls
        .iter()
        .filter(|c| c.outcome == CallOutcome::AlreadyExists)
        .count() as u64;
    totals.idempotent_replays += calls
        .iter()
        .filter(|c| c.outcome == CallOutcome::Replayed)
        .count() as u64;
    totals.creates_sent += inventory.creates;
    totals.lookups_sent += inventory.lookups;
    let adopted = operation
        .steps
        .iter()
        .filter(|s| matches!(s.state, StepState::Done { adopted: true, .. }))
        .count() as u64;
    totals.pre_existing_resources_adopted += seeded;
    totals.resources_adopted_after_lost_reply_or_crash += adopted.saturating_sub(seeded);
}

async fn run_mode(mode: Mode) -> Totals {
    let (logs, _capture) = platform_db::logcheck::capture();
    let mut totals = Totals {
        mode: Some(mode),
        ..Totals::default()
    };
    let mut failures = Vec::new();
    for seed in 0..SCHEDULES {
        if let Err(failure) = run_schedule(seed, mode, &mut totals).await {
            totals.failures += 1;
            if failure.contains("orphans") {
                totals.orphans += 1;
            }
            failures.push(failure);
        }
    }
    if let Ok(dir) = std::env::var("BP_DRILL_REPORT_DIR") {
        let path = format!(
            "{dir}/partial-provision-{}.json",
            serde_json::to_value(mode).unwrap().as_str().unwrap()
        );
        std::fs::write(&path, serde_json::to_string_pretty(&totals).unwrap()).unwrap();
    }
    eprintln!(
        "partial-provision drill: {}",
        serde_json::to_string(&totals).unwrap()
    );
    assert!(
        failures.is_empty(),
        "{} of {SCHEDULES} schedules failed in {mode:?} mode; first: {:?}",
        failures.len(),
        &failures[..failures.len().min(5)]
    );
    assert_eq!(totals.schedules, SCHEDULES);
    assert_eq!(totals.silent_duplicates, 0, "{totals:?}");
    platform_db::logcheck::assert_no_leaks(&logs.text(), &[], "drill logs");
    // The schedules must actually have exercised the dangerous paths.
    assert!(totals.provider_faults_injected > SCHEDULES, "{totals:?}");
    assert!(totals.late_effects_injected > 1_000, "{totals:?}");
    assert!(totals.worker_crashes > SCHEDULES / 2, "{totals:?}");
    assert!(totals.partial_rounds_retried > 100, "{totals:?}");
    totals
}

/// Plain provider: converges on exactly the plan, or reports every duplicate a late create
/// made; never a success with an extra resource.
#[tokio::test]
async fn partial_provisioning_converges_over_10000_fault_schedules() {
    let totals = run_mode(Mode::Plain).await;
    assert!(
        totals.resources_adopted_after_lost_reply_or_crash > 1_000,
        "{totals:?}"
    );
    assert!(
        totals.duplicates_reported_visibly > 0,
        "late creates must be exercised: {totals:?}"
    );
}

/// A provider that de-duplicates by idempotency key: zero duplicates over 10,000 schedules.
#[tokio::test]
async fn with_idempotency_keys_10000_fault_schedules_make_zero_duplicates() {
    let totals = run_mode(Mode::Keys).await;
    assert_eq!(totals.duplicates_reported_visibly, 0, "{totals:?}");
    assert!(
        totals.idempotent_replays > 100,
        "late creates must be replayed: {totals:?}"
    );
}

/// A provider that refuses taken names (`CREATE DATABASE`, bucket names): zero duplicates
/// over 10,000 schedules, through the `AlreadyExists` → look up → adopt path (F6).
#[tokio::test]
async fn with_unique_names_10000_fault_schedules_make_zero_duplicates() {
    let totals = run_mode(Mode::Unique).await;
    assert_eq!(totals.duplicates_reported_visibly, 0, "{totals:?}");
    assert!(
        totals.already_exists_answers > 100,
        "the AlreadyExists path must be exercised: {totals:?}"
    );
}

/// `AlreadyExists` → look up → adopt (F6): a create that landed late makes the retry's
/// create collide; the step adopts the holder instead of failing or duplicating.
#[tokio::test]
async fn already_exists_after_a_late_create_is_adopted() {
    let project = ProjectId::parse("drill").unwrap();
    let desired = DesiredState {
        databases: names("db", 1),
        buckets: Vec::new(),
        identity: None,
    };
    let plan = Plan::build(&project, &desired).unwrap();
    let faults = FaultConfig {
        rules: vec![FaultRule {
            on: CallKind::Create,
            kind: Some(ResourceKind::DatabaseRole),
            nth: 1,
            fault: Fault::LateEffect,
        }],
        random: None,
    };
    let provider = FakeProvider::new(FakeOptions {
        faults,
        unique_names: true,
        record_calls: true,
        ..FakeOptions::default()
    });
    let log = MemLog::default();
    let id = OperationId(Uuid::from_u128(13));
    log.insert(Operation::new(id, 1, &plan));
    let claimed = platform_worker::OperationLog::claim(&log, Uuid::nil(), Duration::from_secs(1))
        .await
        .unwrap()
        .unwrap();
    let outcome = run_claimed(
        &log,
        &provider,
        &platform_worker::NoHooks,
        &config(),
        claimed,
        &mut 1,
    )
    .await
    .unwrap();
    assert_eq!(outcome, OperationOutcome::Succeeded);
    let operation = log.get(id);
    assert!(
        matches!(
            operation.steps[0].state,
            StepState::Done { adopted: true, .. }
        ),
        "{:?}",
        operation.steps[0].state
    );
    assert!(
        provider
            .calls()
            .iter()
            .any(|c| c.outcome == CallOutcome::AlreadyExists)
    );
    check(&plan, &provider, Some(&operation)).unwrap();
}

/// A provider whose name index lags: it answers `AlreadyExists` to the create, but the
/// re-lookup still sees nothing. The step stays uncertain and retries; it never records a
/// resource it has not seen, and never creates blindly (the `Absent` branch, F6).
struct LaggingIndex {
    creates: std::sync::atomic::AtomicU32,
}

impl platform_adapters::Provider for LaggingIndex {
    async fn lookup(
        &self,
        _kind: ResourceKind,
        _name: &platform_domain::ProviderName,
    ) -> Result<platform_domain::Observation, platform_adapters::ProviderError> {
        Ok(platform_domain::Observation::Absent)
    }

    async fn create(
        &self,
        _request: &CreateRequest,
    ) -> Result<platform_domain::ProviderResourceId, platform_adapters::ProviderError> {
        self.creates
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(platform_adapters::ProviderError::AlreadyExists)
    }
}

#[tokio::test]
async fn already_exists_with_no_visible_holder_stays_uncertain_then_gives_up() {
    let project = ProjectId::parse("drill").unwrap();
    let desired = DesiredState {
        databases: Vec::new(),
        buckets: names("files", 1),
        identity: None,
    };
    let plan = Plan::build(&project, &desired).unwrap();
    let provider = LaggingIndex {
        creates: std::sync::atomic::AtomicU32::new(0),
    };
    let log = MemLog::default();
    let id = OperationId(Uuid::from_u128(14));
    log.insert(Operation::new(id, 1, &plan));
    let claimed = platform_worker::OperationLog::claim(&log, Uuid::nil(), Duration::from_secs(1))
        .await
        .unwrap()
        .unwrap();
    let outcome = run_claimed(
        &log,
        &provider,
        &platform_worker::NoHooks,
        &config(),
        claimed,
        &mut 1,
    )
    .await
    .unwrap();
    let operation = log.get(id);
    assert_eq!(outcome, OperationOutcome::Partial);
    assert_eq!(operation.steps[0].state, StepState::GaveUp);
    assert_eq!(
        operation.steps[0].last_error,
        Some(platform_domain::ErrorClass::AlreadyExists)
    );
    assert_eq!(
        provider.creates.load(std::sync::atomic::Ordering::SeqCst),
        u32::from(config().policy.max_attempts)
    );
}

/// The drill can fail: a reconciler that retries a failed create without observing first
/// is caught as duplicates (and orphans) by the same oracle.
#[tokio::test]
async fn the_drill_catches_a_blind_retry() {
    let mut caught = 0;
    for seed in 0..200_u64 {
        let mut rng = seed ^ 0xDEAD_BEEF;
        let project = ProjectId::parse("drill").unwrap();
        let (_, plan) = random_plan(&mut rng, &project);
        let faults = FaultConfig {
            rules: Vec::new(),
            random: Some(RandomFaults {
                seed,
                per_million: 300_000,
                max: Some(10),
            }),
        };
        let provider = FakeProvider::new(FakeOptions {
            faults,
            ..FakeOptions::default()
        });
        for resource in &plan.resources {
            let request = CreateRequest {
                kind: resource.kind,
                name: resource.name.clone(),
                spec: resource.spec.clone(),
                tag: OperationId(Uuid::nil()),
                idempotency_key: String::new(),
            };
            // Blind: "it failed, so it didn't happen" — the bug the contract forbids.
            while provider.create(&request).await.is_err() {}
        }
        if check(&plan, &provider, None).is_err() {
            caught += 1;
        }
    }
    assert!(
        caught > 50,
        "the oracle caught only {caught} of 200 blind-retry runs"
    );
}

/// A resource under the deterministic name that does not match the spec is never
/// overwritten: the operation ends a visible `partial`, the earlier steps keep their real
/// ids, and the stranger is left exactly as found.
#[tokio::test]
async fn a_conflicting_resource_blocks_visibly_and_is_left_alone() {
    let project = ProjectId::parse("drill").unwrap();
    let desired = DesiredState {
        databases: names("db", 1),
        buckets: names("files", 1),
        identity: None,
    };
    let plan = Plan::build(&project, &desired).unwrap();
    let provider = FakeProvider::new(FakeOptions::default());
    let bucket = &plan.resources[2];
    let stranger = provider.seed(
        bucket.kind,
        bucket.name.clone(),
        ResourceSpec::Bucket { quota_bytes: 7 },
    );
    let log = MemLog::default();
    let id = OperationId(Uuid::from_u128(9));
    log.insert(Operation::new(id, 1, &plan));
    let claimed = platform_worker::OperationLog::claim(&log, Uuid::nil(), Duration::from_secs(1))
        .await
        .unwrap()
        .unwrap();
    let outcome = run_claimed(
        &log,
        &provider,
        &platform_worker::NoHooks,
        &config(),
        claimed,
        &mut 1,
    )
    .await
    .unwrap();
    assert_eq!(outcome, OperationOutcome::Partial);
    let operation = log.get(id);
    assert_eq!(operation.status(), "partial");
    assert!(
        operation.steps[0].resource_id().is_some() && operation.steps[1].resource_id().is_some()
    );
    assert_eq!(operation.steps[2].state.status(), "blocked");
    let held: Vec<_> = provider
        .resources()
        .into_iter()
        .filter(|r| r.name == bucket.name)
        .collect();
    assert_eq!(held.len(), 1);
    assert_eq!(
        (held[0].id.clone(), held[0].spec.clone()),
        (stranger, ResourceSpec::Bucket { quota_bytes: 7 })
    );
}

/// A name the provider holds twice is never adopted as if it were one (F5): the
/// operation ends a visible `partial`, the step blocked as a duplicate, both resources
/// left as found for a person to resolve.
#[tokio::test]
async fn a_duplicated_name_is_reported_never_adopted() {
    let project = ProjectId::parse("drill").unwrap();
    let desired = DesiredState {
        databases: names("db", 1),
        buckets: Vec::new(),
        identity: None,
    };
    let plan = Plan::build(&project, &desired).unwrap();
    let provider = FakeProvider::new(FakeOptions::default());
    let role = &plan.resources[0];
    provider.seed(role.kind, role.name.clone(), role.spec.clone());
    provider.seed(role.kind, role.name.clone(), role.spec.clone());
    let log = MemLog::default();
    let id = OperationId(Uuid::from_u128(11));
    log.insert(Operation::new(id, 1, &plan));
    let claimed = platform_worker::OperationLog::claim(&log, Uuid::nil(), Duration::from_secs(1))
        .await
        .unwrap()
        .unwrap();
    let outcome = run_claimed(
        &log,
        &provider,
        &platform_worker::NoHooks,
        &config(),
        claimed,
        &mut 1,
    )
    .await
    .unwrap();
    let operation = log.get(id);
    assert_eq!(
        outcome,
        OperationOutcome::Partial,
        "{:?}",
        operation.steps[0].state
    );
    assert_eq!(operation.steps[0].state.status(), "blocked");
    assert_eq!(provider.inventory().resources, 2, "both left as found");
}

/// A create that lands after its deadline (F5), on a provider with neither unique names
/// nor idempotency keys: the retry's lookup sees the name absent, the late create lands,
/// the retry creates again. The platform cannot prevent that duplicate, but it must never
/// report success with an extra resource: the create is verified, and the duplicate is
/// reported (blocked `duplicate`, operation `partial`).
#[tokio::test]
async fn a_late_create_is_never_reported_as_success() {
    let project = ProjectId::parse("drill").unwrap();
    let desired = DesiredState {
        databases: names("db", 1),
        buckets: Vec::new(),
        identity: None,
    };
    let plan = Plan::build(&project, &desired).unwrap();
    let faults = FaultConfig {
        rules: vec![FaultRule {
            on: CallKind::Create,
            kind: Some(ResourceKind::DatabaseRole),
            nth: 1,
            fault: Fault::LateEffect,
        }],
        random: None,
    };
    let provider = FakeProvider::new(FakeOptions {
        faults,
        ..FakeOptions::default()
    });
    let log = MemLog::default();
    let id = OperationId(Uuid::from_u128(12));
    log.insert(Operation::new(id, 1, &plan));
    let claimed = platform_worker::OperationLog::claim(&log, Uuid::nil(), Duration::from_secs(1))
        .await
        .unwrap()
        .unwrap();
    let outcome = run_claimed(
        &log,
        &provider,
        &platform_worker::NoHooks,
        &config(),
        claimed,
        &mut 1,
    )
    .await
    .unwrap();
    let held = provider.inventory().resources;
    assert!(
        !(outcome == OperationOutcome::Succeeded && held > plan.resources.len()),
        "reported succeeded with {held} resources for {} planned",
        plan.resources.len()
    );
    let operation = log.get(id);
    assert_eq!(outcome, OperationOutcome::Partial);
    assert_eq!(
        operation.steps[0].state,
        StepState::Blocked {
            reason: platform_domain::BlockReason::Duplicate
        }
    );
}
