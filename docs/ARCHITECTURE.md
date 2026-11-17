# Architecture

One control plane reconciling desired configuration against observed
state across mature providers. The design's job is to keep *intent*,
*reality* and *evidence* separable — so a partial or failed operation is
always visible as exactly what it is — and to keep the provider admin
credentials, which are root-equivalent over every project, in as few
places as possible.

## Sources of truth

| Thing | Authority | Notes |
|---|---|---|
| Desired configuration | Control database (PostgreSQL) | Projects, environments, releases, resources, budgets — the only durable intent |
| Operation log | Control database (append-only) | Every provisioning or mutation operation: ID, recorded steps, progress — the reconciliation record |
| Observed state | Providers, reported and recorded in the control DB | Actual resource IDs and health; never assumed from desired |
| Audit | Control database (append-only, enforced by trigger) | Admin actions: principal, authority, target, outcome |
| Secrets | Control database, envelope-encrypted | The key-encryption key lives outside the database (D-015) |
| Application data | The provisioned resources themselves | App databases, buckets and the app's own job table — the control DB never holds app content |
| Running workloads | The container host runtime | Applications keep serving through a control-plane outage |
| Telemetry/history | A separate diagnostics database (D-022) | Bounded retention; losing it never affects apps |
| Cache | None in the first release | Added only for a measured need; never the sole durable job store |

The **control** database is the platform's own store; **application
databases** are provisioned resources under least-privilege roles — a thing
the control plane creates, not a thing it is.

## Components (provisional `platform-` naming)

| Piece | Role |
|---|---|
| `platform-domain` | Pure contract: identities, the desired-state model, the operation/step state machine, resource naming (D-016), budgets, status vocabulary, compatibility rules. No I/O — CI enforces it. |
| `platform-db` | The control store: desired, observed, operations, audit and secrets tables and migrations. |
| `platform-server` | Axum control API and console statics. Writes desired state and operations; **does not link `platform-adapters` and holds no provider credential** — CI checks both. |
| `platform-worker` | The reconciler: claims operations under leases, re-checks each against domain policy and the requesting principal's authority, runs recorded steps through the adapters, writes observed state back. The only process with provider credentials; it opens no listening socket. |
| `platform-adapters` | Provider adapters — PostgreSQL, the S3-compatible store, the OIDC provider (D-019), the container runtime (D-020): each idempotent, bounded, and reporting observed state. One adapter at a time, each behind its own contract, and only after the partial-provisioning drill exists. |
| `platform-cli` | Every control-plane operation from a shell, over the same API. |
| `apps/console` | SvelteKit SPA: management and diagnostics views. Static, served same-origin by the server; holds no authority. |
| `packages/sdk` | SDKs generated from the checked-in OpenAPI document (D-023). |
| `fixtures/` | Fault-injecting fake providers — the substrate of the partial-failure drills and the 1,000,000-request control-API run. |
| `deploy/` | The reproducible self-hosted starter for the single-organization release. |
| `perf/` | Control-API, noisy-neighbor and control-plane-down scenarios and committed results. |

## Request/data paths

**Provision:** desired write → admission check (validation, quotas,
principal authority) → operation and steps committed → the worker claims it
under a lease → re-checks policy → the adapter looks up the deterministic
resource name, creates only if absent, tags it with the operation ID → the
observed resource ID is recorded → next step, or a visible `partial` /
`uncertain` outcome. A retry resumes the same operation and re-checks
observed state — never a blind create.

**Deploy:** release manifest (image pinned by digest) → operation → the
runtime adapter updates the container → observed state reports the running
release. A failed control-plane update leaves already running applications
up where their architecture permits.

**Observe:** the app emits OTLP → the host's collector → the diagnostics
database → API and console read request and job history. Observed-state
reports flow the other way: providers → adapters → control DB → status
views.

**Manage (degraded):** control database, API or worker down → management
calls refuse honestly; running applications are unaffected because the
control plane is not in their request path; the console renders "cannot
manage" with the time of the last observed state, never a green board.

**Upgrade/restore:** check binary and schema compatibility → apply recorded
steps → on restore, restore the control DB and version manifests →
reconcile observed state → **only then** resume or replay jobs. Replaying
before reconciliation is a blind replay of remote effects — the failure
case the contract forbids.

## Consistency and failure model

- **Intent commits before any provider call** — no database transaction
  spans an external call; the operation log is the recoverable record.
- **Operations are claimed under leases** and steps are idempotent under
  the deterministic resource name and the operation ID; a retried step
  re-checks observed state, so partial provisioning reconciles or
  compensates instead of duplicating.
- **Status is a derived join** of desired and observed; `partial`,
  `uncertain` and `unknown` are stored and returned states.
- **Edits carry a revision** so a late observed report cannot silently
  apply against newer intent.
- **Quotas bite at admission and at the workload boundary** (D-017) —
  counters are displays, not the mechanism.
- **Bounds:** every operation, queue, body, retry and budget is limited —
  all numbers are proposed lab targets until measured.

## Control-plane threat model (single organization)

**Scope.** One organization runs its own projects on hosts it controls.
Workloads are written by the organization's developers but are treated as
untrusted with respect to the control plane and to other projects. Hostile
multi-tenant workloads — strangers' code sharing hosts — are out of scope
and need an independently reviewed threat model (D-005, README P-5).

**Assets.** The provider admin credentials (the PostgreSQL provisioner
role, the object-store admin key, the OIDC admin client, access to the
container runtime) — together root-equivalent over every project;
per-project secrets and the key-encryption key; the control database
(desired state, operation log, audit); backups and exports; release
artifacts.

**Actors.** The organization admin (trusted, but audited, and destructive
actions are confirmed); a project developer (a per-project principal); a
workload; a network attacker who reaches the console or API; a compromised
dependency or provider image.

**Trust boundaries.** Browser ↔ API; API ↔ control DB; control DB ↔
worker; worker ↔ providers; workload ↔ host and network; backup storage.

| # | Threat | Mitigation | Test (pass = the named refusal or absence) | From |
|---|---|---|---|---|
| T1 | A compromised console or API reaches every project | The API holds no provider credential and does not link the adapters; the worker re-checks every operation against policy and the requesting principal before any provider call | Architecture check: `platform-server` does not depend on `platform-adapters`; config check: the API's environment and unit file contain no provider secret; `worker_rejects_operation_outside_principal_authority` | B1 |
| T2 | A compromised worker holds the keys | Minimize them: the PostgreSQL provisioner role has `CREATEDB` and `CREATEROLE` but not `SUPERUSER` (PostgreSQL 16+ semantics: it administers only roles it created); the object-store key manages buckets and policies only; the runtime is rootless under a dedicated user (D-014); the worker opens no listening socket | `provisioner_role_is_not_superuser`; `worker_listens_on_nothing`; a recorded review of each adapter's credential scope | B2 |
| T3 | Cross-project access by a developer or workload | Each project role owns only its database and `CONNECT` is revoked from `PUBLIC`; storage keys are scoped to one bucket; one OIDC realm per project; one container network per project with no route to the control DB, provider admin endpoints or other projects | `project_a_role_cannot_connect_to_project_b_db`; `project_a_key_cannot_read_project_b_bucket`; `project_a_token_rejected_by_project_b`; `workload_cannot_reach_control_db_or_admin_endpoints`; `workload_cannot_reach_other_project` | B2–B3 |
| T4 | Container escape or host takeover from a workload | Rootless runtime; the runtime socket is never mounted into a workload; privileged containers and host mounts are refused at admission; read-only root filesystem by default; default seccomp profile; capabilities dropped | `privileged_or_host_mount_refused_at_admission`; `no_workload_mounts_runtime_socket` | B3 |
| T5 | Admin abuse or repudiation | Append-only audit (trigger refuses `UPDATE`, `DELETE`, `TRUNCATE`) with principal, authority, target and outcome; destructive operations (delete project, restore, key rotation) need typed confirmation and a fresh re-authentication | `audit_is_append_only`; `destructive_op_without_reauth_refused` | B1 |
| T6 | Secret disclosure | Envelope encryption with a key outside the control DB; secrets are write-only in the API and absent from operation steps and logs (D-015) | Log-capture test over every drill; `secrets_are_write_only_in_api`; `step_records_contain_no_secret` | B1 |
| T7 | Server-side request forgery through user-supplied URLs (OIDC redirect URIs, webhook or registry URLs) | Adapters connect only to configured provider endpoints; user-supplied URLs are validated and stored, and anything the platform must fetch goes through an egress guard that refuses loopback, RFC 1918, link-local and metadata addresses, pins DNS answers and revalidates redirects | Egress tests: link-local, RFC 1918, loopback, DNS rebinding, redirect to a private target — each refused before connecting | B2 |
| T8 | Supply chain | Provider images pinned by digest; `cargo deny` and `cargo audit` in CI; release provenance and a vulnerability process (B7) | CI job fails on an unpinned image or a known advisory without a recorded exception. B0 implements this with `scripts/check-pins.sh` (images by digest, actions by commit SHA; shown able to fail) and `cargo deny` (advisories, daily) | B0 |
| T9 | Backup theft or a malicious restore | Backups encrypted at rest with restricted permissions; restore needs re-authentication; restore is followed by reconciliation before any job resumes | Restore drill (failure case 4, point c) | B5 |
| T10 | Control-plane denial of service | Bounded bodies, per-principal rate limits, admission caps; running apps are unaffected because the control plane is not in their path | `control-plane-down` drill; overload scenario refuses with 429/503, never hangs | B3 |

**Accepted risks (stated, not hidden).** The organization admin can do
anything — this is a single-organization tool. The worker host remains the
crown jewel: compromise of it is compromise of every project, which is why
it holds no listener and is the only place credentials exist.

## Repo layout

B0 built `platform-domain`, `platform-db`, `platform-adapters` (the port
and the HTTP adapter for the fake), `platform-worker`, `platform-server`,
`fixtures/fake-provider`, `deploy/compose.yaml` (the development stack, not
the starter), `perf/` and `scripts/`. The rest is planned. The reconciler's
write protocol is D-029, and the fake provider's semantics are D-030.

```
crates/      platform-domain, platform-db, platform-server,
             platform-worker, platform-adapters, platform-cli
apps/        console (SvelteKit)
packages/    generated SDK(s)
deploy/      self-hosted starter (single organization)
fixtures/    fault-injecting providers
perf/        load scenarios + results (machine + commit)
docs/        END_GOAL, ARCHITECTURE, ROADMAP, DECISIONS;
             modules/ is added when the first module lands
scripts/     check.sh (what CI runs), drills
```

## Failure cases

The four contract cases, with their drills and pass conditions, live in
`END_GOAL.md`; the threat-model tests above are required alongside them.
The partial-provisioning drill runs against the fake provider from B0 and
against every real adapter before a second adapter is added, per the guide.
