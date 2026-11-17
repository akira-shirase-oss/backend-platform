# Backend Platform

> **Status — parked (future horizon); B0 in progress: built and green, two
> items owed.** The maintainer asked on 2026-11-16 for development to start
> on every planned project, so B0 was built:
>
> - the evidence memo (`docs/B0_MEMO.md`);
> - the engineering spine, with CI;
> - the partial-provisioning drill;
> - the first load scenario.
>
> The memo's recommendation (stay parked, and build a copyable starter
> template instead of a control plane) was **accepted with corrections on
> 2026-11-16** (D-036). **B1-T, the starter template, is in progress**
> (`template/`, `docs/STARTER_TEMPLATE.md`): items 1–10 are built and
> tested, the operator guide runs as written on a CI VM, and Durable
> Runner's acceptance run on a droplet is owed (`docs/B1T_ADOPTION.md`).
> The control-plane milestones B1–B7 do not start until the maintainer
> unparks the project (P-1). Still owed for B0: a quiet-machine run of the 100,000-request
> baseline and the 1,000,000-request run. Both smokes so far ran on a busy
> laptop and missed their thresholds; both are kept. The guide marks this
> an expansion horizon, not an approved simultaneous build or a commercial
> offering.

A self-hosted backend platform in the Appwrite/Supabase direction — one
developer experience over deployment, identity, data, files, jobs and
diagnostics — assembled from proven components rather than rewritten
infrastructure. The point is to make small applications easier to
*operate*, not merely easier to scaffold: provisioning is a reconciled
process, running applications outlive the control plane, and upgrade,
restore and export are rehearsed drills, not runbook folklore.

**What it is not:** a shared login or account service for the existing
independent products. Those products keep their own accounts and
databases and are not tenants of this platform (D-011). Identity the
platform provisions belongs to one project at a time.

## Blocked on

| # | Needed | From | Blocks |
|---|---|---|---|
| P-1 | The decision to unpark the project (B0 alone was started, at the maintainer's request, on 2026-11-16) | the maintainer | the control-plane milestones B1–B7 |
| P-2 | Resolved 2026-11-16: the B0 memo was accepted with corrections (D-036) | — | nothing |
| P-3 | A second developer who has not worked on the platform | the maintainer, then that person | the B5 gate |
| P-4 | Three independent teams to run the operations script (D-028) | outside parties | the B7 ship gate |
| P-5 | An independently reviewed threat model for hostile multi-tenant workloads | an outside reviewer | any shared-hosting direction (not the first release) |

## What exists (B0)

| Piece | What it is | Proven by |
|---|---|---|
| `crates/platform-domain` | deterministic provider names (D-016), the plan, the operation/step state machine; no I/O | unit and property tests; `scripts/check-architecture.sh` (shown able to fail) |
| `crates/platform-db` | the control-DB operation log on PostgreSQL: admission bounds, idempotency keys, leases with fencing tokens, one-statement step writes, an append-only event log that the runtime role cannot rewrite (D-034), observed resources; `platform-dbctl` bootstraps the roles and migrates | `crates/platform-db/tests/log.rs`, `privileges.rs` |
| `crates/platform-adapters` | the provider port and the HTTP adapter; linked only by the worker | architecture check: `platform-server` links no adapter (T1) |
| `crates/platform-worker` | the reconciler: observe before every create, intent committed before every provider call | drill `partial-provision` (below) |
| `crates/platform-server` | the control API: desired state, operations, status; 503 `control_plane_unavailable` when the control DB is down | `crates/platform-server/tests/api.rs` |
| `fixtures/fake-provider` | fault-injecting fake provider, in process and over HTTP; allows duplicate names, so blind retries show | its unit tests; the drills |
| drill `partial-provision` | 10,000 randomized fault schedules in process, plus the real worker killed with SIGKILL at every step | `partial_provision.rs`, `kill_drill.rs`: 0 duplicates, 0 orphans |
| `perf/scenarios/control-api.js` | 80% reads / 20% desired-state writes at a constant arrival rate, the worker reconciling under 0.1% provider faults | `perf/results/` (busy-machine smokes, kept); the full run is owed |

## What exists (B1-T, the starter template)

`template/` is a copyable Rust workspace that a product copies once and owns
(`scripts/copy-template.sh DEST NAME`; `template/README.md` starts with a quickstart).

| Piece | Proven by |
|---|---|
| the bounded accept loop and middleware stack (`starter-http`) | `tests/limits.rs`; each of 16 limits removed in turn fails its test (`template/scripts/prove-limits.sh`, CI job `template-limits`) |
| owner and runtime roles, the protections manifest, the schema gate (`starter-db`) | 13 weakenings, each refused; a foreign schema refused (exit 4) |
| the copy-and-own accounts kit (`starter-accounts`) | isolation across workspaces, plan limits under concurrency, sign-in budgets against floods, deletion, the log capture |
| the health contract, configuration from credentials (`starter-server`) | the real binary's tests; `/readyz` honest within its deadline |
| Caddy, the hardened unit (exposure 1.4), `harden.sh`, deploy and rollback | the operator guide run as written on Ubuntu 24.04 with systemd (CI job `template-install`) |
| restore drills (PostgreSQL, SQLite, journal; Neon and bucket written, not run) and the upgrade rehearsal | in the template's gate, with controls; the journal drill against Durable Runner's binaries |
| the copy step | a renamed copy builds and passes (CI job `template-copy`) |

Commands (Rust 1.99.1 through rustup; see `AGENTS.md`):

```
scripts/stack.sh up            # PostgreSQL on 127.0.0.1:61432; roles and the bp_dev database, migrated
scripts/check.sh               # what CI runs: fmt, clippy, architecture and pin checks, tests, drills, load probe, cargo deny
MODE=smoke scripts/load.sh     # the 100,000-request baseline (MODE=full: 1,000,000, quiet machine only)
```

## Documents

- `docs/B0_MEMO.md` — starter template or running control plane? The
  evidence, the threat-surface comparison, verified component versions and
  the recommendation (stay parked), accepted with corrections (D-036)
- `docs/STARTER_TEMPLATE.md` — B1-T: what the copy-and-own starter template
  contains, each item's status and proof, and its acceptance test
- `docs/B1T_ADOPTION.md` — the acceptance test's dry run: Durable Runner and
  the template, what ran and what adoption changes
- `template/README.md` — the template itself, from its quickstart
- `docs/END_GOAL.md` — what done means, scope, invariants, the four
  required failure cases with their drills and pass conditions, the ship
  gate in testable form
- `docs/ARCHITECTURE.md` — sources of truth, components, paths,
  consistency, and the control-plane threat model
- `docs/ROADMAP.md` — B0, B1-T and B1–B7 with per-milestone proof gates
- `docs/DECISIONS.md` — the guide-seeded trade-offs (D-001–D-010), the
  decisions made before code (D-011–D-028), those made in B0 and its review
  round (D-029–D-036), those made in B1-T (D-037 onward) and the questions
  still open
- `AGENTS.md` — project rules on top of the workspace rules

Contract: the engineering guide `09_Backend_Platform.docx` (kept in the
maintainer's workspace under `docs/Project_Guides/`, not in this
repository); it wins on conflict.

**What counts as done:** the ship gate in `docs/END_GOAL.md` — three
independent teams run the operations script from the documentation alone,
faster than the same operations assembled by hand — with the four contract
failure cases proven and the load results committed with machine + commit.
