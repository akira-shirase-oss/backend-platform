# Architecture

One control plane reconciling desired configuration against observed
state across mature providers. The design's job is to keep *intent*,
*reality* and *evidence* separable — so a partial or failed operation is
always visible as exactly what it is.

## Sources of truth

| Thing | Authority | Notes |
|---|---|---|
| Desired configuration | Control database (PostgreSQL) | Projects, environments, releases, resources, budgets — the only durable intent |
| Operation log | Control database (append-only) | Every provisioning/mutation operation: ID, recorded steps, progress — the reconciliation record |
| Observed state | Providers, reported + recorded in the control DB | Actual resource IDs and health; never assumed from desired |
| Audit | Control database (append-only) | Admin actions: principal, authority, target, outcome |
| Application data | The provisioned resources themselves | App databases and storage paths — the control DB never holds app content |
| Running workloads | The container host runtime | Applications keep serving through a control-plane outage |
| Telemetry/history | Diagnostics store (open question) | Request/job history for the observe and diagnose paths |
| Cache | Disposable accelerator | Only where evidence justifies it; never the sole durable job store |

Two different PostgreSQL roles must not be conflated: the **control**
database is the platform's own store; **application databases** are
provisioned resources under least-privilege roles — a thing the control
plane creates, not a thing it is.

## Components (provisional `platform-` naming)

| Piece | Role |
|---|---|
| `platform-domain` | Pure contract: identities, the desired-state model, the operation/step state machine, budgets, status vocabulary, compatibility rules. No I/O — enforced. |
| `platform-db` | The control store: desired/observed/operations/audit tables and migrations. |
| `platform-server` | Axum control API + console statics; `--role api\|worker\|all`. |
| `platform-worker` | The reconciler: claims operations under leases, runs recorded steps through adapters, writes observed state back. |
| `platform-adapters` | Provider adapters (PostgreSQL, S3-compatible, OIDC, container runtime, job engine): each idempotent, bounded, and reporting observed state. The set grows one adapter at a time, each behind its own contract — and only after the partial-provisioning drill exists. |
| `platform-cli` | Every control-plane operation from a shell. |
| `apps/console` | SvelteKit SPA: management + diagnostics views. Static, served same-origin by the server; holds no authority. |
| `packages/sdk` | Generated client SDK(s); the generation source is an open question. |
| `fixtures/` | Fault-injecting fake providers — the substrate of the partial-failure drills. |
| `deploy/` | The reproducible self-hosted starter for the single-organization release. |
| `perf/` | Control-API, noisy-neighbor and control-plane-down scenarios + committed results. |
| `docs/modules/` | Per-module contracts as they land. |

## Request/data paths

**Provision:** desired write → admission check (validation + quotas) →
operation + steps committed → worker claims under lease → adapter applies
(one provider call, one step record) → observed resource IDs recorded →
next step, or a visible `partial`/`uncertain` outcome. A retry resumes the
same operation ID and re-checks observed state — never a blind create.

**Deploy:** release manifest → operation → the runtime is updated with the
pinned artifact → observed state reports the running release. A failed
control-plane update leaves already running applications up where their
architecture permits.

**Observe:** the app emits telemetry → diagnostics store → API/console
reads request/job history. Observed-state reports flow the other way:
providers → adapters → control DB → status views.

**Manage (degraded):** control database or control plane down →
management calls refuse honestly; running applications are unaffected;
the console renders "cannot manage", never a fabricated green board.

**Upgrade/restore:** restore the control DB + version manifests →
reconcile observed state → **only then** resume or replay jobs. Replaying
before reconciliation is a blind replay of remote effects — the failure
case the contract forbids.

## Consistency and failure model

- **Intent commits before any provider call** — no database transaction
  spans an external call; the operation log is the recoverable record.
- **Operations are claimed under leases** and steps are idempotent under
  the operation ID; a retried step re-checks observed state, so partial
  provisioning reconciles or compensates instead of duplicating.
- **Status is a derived join** of desired and observed; `partial`,
  `uncertain` and `unknown` are stored and returned states, never
  smoothed into a green badge.
- **Edits carry a revision** so a late observed report cannot silently
  apply against newer intent (mechanism pinned in the module docs).
- **Quotas bite at admission and at the workload boundary** — enforcement
  lives below the dashboard; counters are displays, not the mechanism.
- **Admin actions are audited** with principal and explicit authority.
- **Bounds:** every operation, queue, body, retry and budget is limited —
  all numbers are proposed lab targets until measured.

## Repo layout (planned)

```
crates/      platform-domain, platform-db, platform-server,
             platform-worker, platform-adapters, platform-cli
apps/        console (SvelteKit)
packages/    generated SDK(s)
deploy/      self-hosted starter (single organization)
fixtures/    fault-injecting providers
perf/        load scenarios + results (machine + commit)
docs/        END_GOAL, ARCHITECTURE, ROADMAP, DECISIONS, modules/
scripts/     gates, drills
```

## Failure cases

The four contract cases live in `END_GOAL.md`: partial provisioning, the
noisy neighbor, cache/control-plane loss, interrupted upgrade/restore.
Each lands as a fixture drill in the milestone that makes it possible —
and the partial-provisioning drill runs before any second adapter is
added, per the guide.
