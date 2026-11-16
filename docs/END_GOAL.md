# Backend Platform — end goal

**Make small applications easier to operate, not merely easier to scaffold.**

A self-hosted control plane that connects deployment, identity, data,
files, jobs and diagnostics into one developer experience — the
Appwrite/Supabase direction, reached by assembling proven components and
mature dependencies rather than cloning a product or rewriting databases,
cryptography or payments. The first useful release is a reproducible
self-hosted starter for **one** application pattern — identity, a
database, files, one background job and diagnostics — run by a single
organization with explicit per-project isolation.

Derived from `../../docs/Project_Guides/09_Backend_Platform.docx` (the
contract); where this file and the guide disagree, the guide wins. The
guide marks this an **expansion horizon**: planning-stage work, not an
approved simultaneous build and not a commercial offering.

## In scope (the first useful release)

- The control plane: project, environment, release and resource lifecycle
  services (Rust + Axum + Tokio).
- Reconciled provisioning: desired configuration in the control database,
  observed state reported back, operations with IDs and recorded steps.
- One application pattern deployed as standard containers onto provisioned
  resources: PostgreSQL, a reviewed OIDC integration, S3-compatible object
  storage, one background job.
- Diagnostics: request/job history sufficient to observe and diagnose that
  one pattern.
- Access and isolation: per-project principals, least-privilege database
  roles, scoped storage paths, isolated secrets, network boundaries;
  audited admin actions.
- Interfaces: the SvelteKit console, the Rust CLI, generated SDKs.
- Upgrade and exit: version manifests, a compatibility matrix, consistent
  backups and export — rehearsed as drills, not documented as hopes.

## Out of scope (explicit exclusions — each needs separate evidence, not a flag)

- A hosted multi-tenant service, metered billing, regions.
- Hostile multi-tenant workloads — these require an independently reviewed
  threat model before any shared-hosting claim.
- Rewritten infrastructure: databases, cryptography and payments are
  mature dependencies, always.
- The custom JavaScript runtime (jsrt) as an executor — optional, later.
- Native apps — optional separate clients.
- An SLA or public shared-hosting claim — needs an operating team and the
  capacity/isolation evidence first.
- Breadth for its own sake: a larger dashboard alone is not success.

## Non-negotiable invariants

1. **Desired ≠ observed.** The control database stores desired
   configuration; actual resources report observed state. Status is
   derived from both, and `partial`, `uncertain` and `unknown` are
   first-class outcomes — shown, never smoothed over.
2. **Provisioning is reconciled, never atomic.** Operations carry IDs and
   recorded steps; retries resume the same operation and re-check observed
   state. Reconcile or compensate — never a blind retry that creates a
   second database.
3. **Running applications outlive the control plane.** A failed control
   UI or control plane must not stop already running applications;
   management degrades honestly instead of displaying fabricated success.
4. **No dependency trap.** Products run independently before integration;
   shared contracts are limited to identity, IDs, events and health; no
   shared mutable database across unrelated products.
5. **Least privilege per project.** Principals, database roles, storage
   paths, secrets and network boundaries are scoped to a project; admin
   actions carry explicit authority and an audit row.
6. **Bounded everything.** Every operation, queue, body, retry loop and
   budget has a limit and a deadline. No database transaction spans a
   provider call — intent commits to the operation log first.
7. **Cache is disposable.** An accelerator only where evidence justifies
   it; never the sole durable job store, never an authority.
8. **Honest upgrade and exit.** Version manifests and a compatibility
   matrix; consistent backups and export; reconcile after restore before
   replaying jobs — no blind replay of remote effects.

## Required failure cases

| Trigger | Required result |
|---|---|
| Provisioning partially succeeds | Keep the actual resource IDs; reconcile or compensate safely — never create duplicate databases on a blind retry |
| One tenant overloads resources | Enforce quotas and isolate workloads at the enforcement layer; dashboard-only counters do not count |
| Cache or control plane unavailable | Durable data and running services stay intact; management shows degraded, never fabricated success |
| Upgrade or restore interrupted | Recorded steps, compatible binaries and tested recovery — no blind replay of remote effects |

## Ship gate

From the guide: **advance only when integrations save independent
developers setup and recovery effort; a larger dashboard alone is not the
success criterion.** Concretely: three independent teams repeat install,
provision, deploy, observe, upgrade, restore and export from the
documentation — and their observed friction, not our confidence, decides
what expands next.

## Evidence plan

Measure what the guide names: time to useful deployment, control-API
latency, idle resource footprint and upgrade disruption. Ten projects
under mixed reads/jobs with one noisy neighbor and explicit per-project
budgets; record what remains available when the control plane stops.
Cross-tenant negative tests and separately measured workload isolation
precede any shared-hosting conversation — a high aggregate requests/s
number is not isolation evidence. Load runs follow the workspace standard
(million-scale where a traffic unit exists; here the units are
control-plane operations, app-facing requests and provisioning runs);
results are committed with machine spec + commit hash, misses kept. Every
number in these docs is a proposed lab target until a run produces
evidence.

## Open questions (the guide leaves these open — decide from evidence, not preference)

- **The application pattern.** The guide's first step is to choose one
  recurring developer problem *demonstrated by existing projects* — the
  pattern is selected at B0 from evidence in this workspace, not picked
  here.
- **OIDC provider and review bar.** "Reviewed OIDC integration" names no
  provider and no acceptance criteria; both are decided before identity
  ships.
- **Container runtime and orchestration.** "Standard application
  containers" does not name a runtime or a host model (single host?
  compose? a scheduler?); decided with the deployment adapter's contract.
- **Job engine.** An in-workspace durable-runner integration versus a
  mature external queue is undecided; either way the cache is never the
  durable job store.
- **Diagnostics storage.** Request/job history presumes an OpenTelemetry
  pipeline (the guide's concept list), but the collector topology and
  store are open.
- **SDK generation source.** "Generated SDKs" presumes a machine-readable
  API contract; the source (OpenAPI or otherwise) is undecided.
- **"Safe AI workflows."** Named as a direction with no contract; no
  agent-facing feature lands without one (the colophon precedent: agents
  propose, the platform enforces).
- **"Ten projects" / "tenant".** The load plan's tenancy is ambiguous —
  presumed to be ten projects inside the single organization, with
  "tenant" meaning project, until confirmed.
