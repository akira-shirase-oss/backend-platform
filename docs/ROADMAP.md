# Roadmap

**Parked (future horizon).** Every milestone is *Planned* and nothing
starts until the maintainer unparks the project (README, P-1). When it
starts, each milestone begins only when the previous gate's evidence
exists — one milestone per branch, merged when its proof is stored. The
guide's four implementation steps map as: **1** integration case → B0 ·
**2** starter deployment → B1–B4 · **3** operations → B5 · **4** external
validation → B7. The performance and load plan is B0 (first scenario) and
B6 (full runs).

**Status rule.** A status cell may say *Done* only when it names the tests
and the evidence path that prove the gate. Anything else is *Planned*,
*In progress* or *Blocked (P-n)*. Load evidence measured on a busy machine
is a smoke observation, never capacity.

Every milestone ships: regression tests; its failure drills and
threat-model tests as automated tests in CI; a load scenario with
thresholds for anything new that serves traffic; the log-capture test over
its drills.

| Milestone | Builds | Proves (gate) | Status |
|---|---|---|---|
| **B0 — evidence memo and spine** | The evidence memo (D-012): the recurring developer problem in existing projects, the identity question answered first (D-011), the starter-template alternative compared; the Cargo workspace; `platform-domain` operation and step state machine and resource naming (D-016); the control-DB operation log; the fault-injecting fake provider; `scripts/check.sh` and a CI job running it (fmt, clippy `-D warnings`, tests, architecture checks, `cargo deny`); the first k6 scenario (create and read operations against the fake provider) | The memo is accepted by the maintainer, or it recommends staying parked and the project stays parked; CI green from the first commit; **drill `partial-provision` against the fake provider** passes over 10,000 randomized fault schedules with zero duplicates; a 100,000-request smoke of the control-API scenario committed as the baseline | Blocked (P-1) |
| **B1 — control plane core** | Control DB (desired, observed, operations, audit, secrets), project and environment lifecycle API, per-project principals, envelope encryption (D-015), console shell, CLI skeleton, the OpenAPI document and its drift check (D-023) | One API for console and CLI; the desired/observed split visible in every status response; T1, T5 and T6 tests pass (no provider credential in the API, append-only audit, re-authentication for destructive operations, write-only secrets) | Planned |
| **B2 — reconciled provisioning** | The lease-claimed worker; the starter's adapters — PostgreSQL database and role, an S3-compatible bucket with enforced quotas, an OIDC realm and client (D-019); observed-state reporting; the egress guard | **`partial-provision` against the real adapters** (smaller count, reported separately) with zero duplicates; T2, T3 (database, storage and identity halves) and T7 tests pass; the real-provider count of 1,000 provision and deprovision cycles recorded; the OIDC provider's idle footprint measured. This drill gates every later adapter | Planned |
| **B3 — starter deployment** | Release manifests with digest-pinned images, the deploy operation, the rootless container runtime adapter (D-020), per-project networks; the one application pattern end to end: create → provision → deploy → run | The starter runs; **drill `control-plane-down`** (failure case 3) passes and the availability matrix is committed; T3 (network half), T4 and T10 tests pass | Planned |
| **B4 — jobs and diagnostics** | The one background job on the app's own database (D-021); the OTLP collector and diagnostics database (D-022); diagnosis views in console and CLI; quota enforcement per resource (D-017) | The observe → diagnose path works on a seeded failure; **drill `noisy-neighbor`** (failure case 2) passes on the enforcement counters | Planned |
| **B5 — operate** | Upgrade with version manifests and a compatibility matrix, secret rotation, backup, restore and export — as tested drills | **Drill `interrupted-upgrade-restore`** (failure case 4) passes at all four points (D-027); T9 passes; a second developer runs upgrade, rotate, restore and export from the docs alone (P-3) | Planned |
| **B6 — load and isolation evidence** | The full runs (D-018): 1,000,000 control-API requests against the fake providers; 1,000,000 app-facing requests across ten projects with one noisy neighbor; time to useful deployment, idle footprint, upgrade disruption | Results committed with machine, commit and machine load; misses kept, not rerun; budgets hold at the enforcement layer | Planned |
| **B7 — external validation and ship gate** | Install and operate docs hardened by use; the supported component matrix, artifact provenance, the vulnerability process, migration paths; the baseline timing of the operations script by hand | The ship gate in `END_GOAL.md` (D-028): three independent teams complete the operations script with no founder intervention, faster than the baseline | Blocked (P-4) |

## Later directions (each needs its own contract and evidence)

Hosted multi-tenancy, metered billing, regions and stronger isolation
(an independently reviewed threat model first, P-5), the JavaScript runtime
as an executor, native apps, additional adapters, optional adapters to
other independent products through their public APIs, and "safe AI
workflows", which has no contract at all. Expand only from observed need.
