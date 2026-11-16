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

Derived from the engineering guide `09_Backend_Platform.docx` (outside this
repository); where this file and the guide disagree, the guide wins. The
guide marks this an **expansion horizon**: the project is parked, and it is
not a commercial offering. Every number below is a proposed lab workload or
acceptance target until a run produces evidence.

## In scope (the first useful release)

- The control plane: project, environment, release and resource lifecycle
  services (Rust + Axum + Tokio).
- Reconciled provisioning: desired configuration in the control database,
  observed state reported back, operations with IDs and recorded steps,
  deterministic provider resource names (D-016).
- One application pattern, chosen at B0 from a recurring problem in
  existing projects (D-012), deployed as standard containers on one host
  (D-020) onto provisioned resources: a PostgreSQL database, an OIDC realm
  for that project (D-019), an S3-compatible bucket, one background job
  (D-021).
- Diagnostics: request and job history for that one pattern (D-022).
- Access and isolation: per-project principals, least-privilege database
  roles, scoped storage paths, isolated secrets, network boundaries;
  audited admin actions; the control-plane threat model in
  `ARCHITECTURE.md` with a test per mitigation.
- Interfaces: the SvelteKit console, the Rust CLI, SDKs generated from the
  OpenAPI document (D-023).
- Upgrade and exit: version manifests, a compatibility matrix, consistent
  backups and export — rehearsed as drills.

## Out of scope (each needs separate evidence, not a flag)

- A hosted multi-tenant service, metered billing, regions.
- Hostile multi-tenant workloads — these require an independently reviewed
  threat model (P-5) before any shared-hosting claim.
- Being the login, account or workspace service for the existing
  independent products (D-011).
- Rewritten infrastructure: databases, cryptography and payments are
  mature dependencies, always.
- The custom JavaScript runtime as an executor — optional, later.
- Native apps — optional separate clients.
- "Safe AI workflows" — no contract exists, so no feature (D-024).
- An SLA or public shared-hosting claim — needs an operating team and the
  capacity and isolation evidence first.
- Breadth for its own sake: a larger dashboard alone is not success.

## Non-negotiable invariants

1. **Desired ≠ observed.** The control database stores desired
   configuration; actual resources report observed state. Status is
   derived from both, and `partial`, `uncertain` and `unknown` are
   first-class outcomes — shown, never smoothed over.
2. **Provisioning is reconciled, never atomic.** Operations carry IDs and
   recorded steps; provider resource names are derived from the project and
   resource, so a retry finds what exists instead of creating a second one
   (D-016). Reconcile or compensate — never a blind retry.
3. **Running applications outlive the control plane.** The control plane
   is not in any application's request path; a failed control UI, API,
   worker, database or cache does not stop running applications, and
   management degrades honestly instead of displaying fabricated success.
4. **No dependency trap.** Products run independently before integration.
   What may be shared is a small set of *contracts* — standard OIDC
   tokens, ID formats, an event envelope, a health-endpoint shape — never a
   shared login service or a shared mutable database across unrelated
   products (D-004, D-011).
5. **Least privilege per project, and none in the API.** Principals,
   database roles, storage paths, secrets and network boundaries are scoped
   to a project; admin actions carry explicit authority and an append-only
   audit row. Provider admin credentials live only in the reconciler
   worker; the API process holds none (D-013).
6. **Bounded everything.** Every operation, queue, body, retry loop and
   budget has a limit and a deadline. No database transaction spans a
   provider call — intent commits to the operation log first.
7. **Cache is disposable.** An accelerator only where evidence justifies
   it; never the sole durable job store, never an authority. The first
   release has none.
8. **Honest upgrade and exit.** Version manifests and a compatibility
   matrix; consistent backups and export; reconcile after restore before
   replaying jobs — no blind replay of remote effects.
9. **Secrets stay secret.** Envelope-encrypted with a key outside the
   control database, write-only in the API, absent from logs and step
   records — proven by a log-capture test (D-015).

## Required failure cases (contract, not options)

| Trigger | Required result | Proven by | Pass condition | From |
|---|---|---|---|---|
| Provisioning partially succeeds | Keep the actual resource IDs; reconcile or compensate safely — never create duplicate databases on a blind retry | Drill `partial-provision`: the fault-injecting fake provider fails after any step, loses the reply to a create that succeeded, or the worker is killed mid-step; 10,000 randomized fault schedules, then the same faults against the real adapters on a smaller count | Exactly one provider resource per desired resource (counted on the provider side); every operation ends `succeeded`, `compensated` or a visible `partial` with the real resource IDs recorded; zero duplicates across all schedules | fakes B0, real adapters B2 |
| One tenant overloads resources | Enforce quotas and isolate workloads; dashboard-only counters do not count | Drill `noisy-neighbor`: ten projects on one host, one driven at 5× its budget for CPU, memory, connections, storage and jobs (D-017) | The other nine projects' p95 latency stays within 1.25× of their solo baseline; the test reads the *enforcement* counters — cgroup throttling and memory events, refused Postgres connections and cancelled statements, object-store quota errors, platform admission refusals — and each shows the noisy project being limited; no assertion reads a dashboard number | B4 |
| Cache or control plane unavailable | Keep durable data and running services intact; show degraded management rather than fabricated success | Drill `control-plane-down`: stop the control database, the API, the worker and (if one exists) the cache, one at a time and all together, while the ten starter apps serve traffic | App-facing error rate and p95 unchanged from the baseline within run-to-run noise; app data checksums identical before and after; management calls answer `503 control_plane_unavailable` and the console shows "cannot manage" with the time of the last observed state; the availability matrix (what still works) is committed | B3 |
| Upgrade or restore is interrupted | Use recorded steps, compatible binaries and tested recovery; no blind replay of remote effects | Drill `interrupted-upgrade-restore` at four named points (D-027): (a) after the control-DB migration, before the binary swap; (b) during the binary swap; (c) after the control-DB restore, before reconciliation; (d) after reconciliation, before jobs resume | Re-running the command resumes from the recorded step; the compatibility check refuses an incompatible binary and schema pair; the operation log shows reconcile completing before any job resumes; the fake provider records zero repeated remote effects; running apps keep serving through (a) and (b) | B5 |

The threat-model tests in `ARCHITECTURE.md` are required alongside these;
each lands in the milestone that builds what it protects.

## Ship gate

The guide: **advance only when integrations save independent developers
setup and recovery effort; a larger dashboard alone is not the success
criterion.** In testable form (D-028), at B7:

1. **Independent** means developers who did not write the platform, from
   three different teams, working from the published documentation with no
   other help. Any intervention by the maintainer fails that operation for
   that team.
2. **The operations script:** install; create a project; provision a
   database, files, identity and the one job; deploy version 1; observe a
   request and a job; diagnose a seeded failure; upgrade the platform one
   minor version; rotate a project secret; restore from backup; export the
   project.
3. **The baseline comes first:** the same script, done by hand with the same
   upstream components and their own documentation, timed before any team
   uses the platform.
4. **Pass:** all three teams complete every operation; the median total
   time with the platform is lower than the baseline, and each recovery
   operation (upgrade, rotate, restore) is individually faster; every help
   request and friction note is recorded and decides what expands next.

Blocked on the teams (README, P-4). A miss is a recorded outcome, not
something to re-run until it passes.

## Evidence plan

Measure what the guide names: time to useful deployment, control-API
latency, idle resource footprint and upgrade disruption; ten projects under
mixed reads and jobs with one noisy neighbor and explicit per-project
budgets; what remains available when the control plane stops. Cross-tenant
negative tests and separately measured workload isolation precede any
shared-hosting conversation — a high aggregate requests/s number is not
isolation evidence.

The 1,000,000-unit runs (D-018) count:

- **control-API requests** — 1,000,000 requests (80% reads, 20%
  desired-state writes that create operations) against the platform, with
  the worker reconciling against the fault-injecting fake providers;
- **app-facing requests** — 1,000,000 requests across the ten starter
  projects during the noisy-neighbor drill.

A real-provider count — 1,000 provision and deprovision cycles against
local PostgreSQL, the object store and the OIDC provider — is reported
separately and never extrapolated. A million real `CREATE DATABASE` runs is
not a meaningful workload, and no hosted third-party service is ever
load-tested. Results are committed under `perf/results/` with machine spec,
commit hash and machine load; misses are kept; laptop numbers are
development observations.
