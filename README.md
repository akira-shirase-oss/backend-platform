# Backend Platform

> **Status — parked (future horizon); B0 in progress.** The maintainer asked
> on 2026-11-16 for development to start on every planned project, so
> milestone B0 — the evidence memo, CI, the fake provider, the first load
> scenario and the partial-provisioning drill — is under way. The project
> stays parked: nothing past B0 starts until the maintainer unparks it (P-1)
> and accepts the B0 memo (P-2). The guide marks this an expansion horizon,
> not an approved simultaneous build or a commercial offering. See
> `docs/ROADMAP.md`.

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
| P-1 | The decision to unpark the project | the maintainer | B0 and everything after it |
| P-2 | Acceptance of the B0 memo, including its answer to "starter template or platform?" (D-012) | the maintainer | B1 |
| P-3 | A second developer who has not worked on the platform | the maintainer, then that person | the B5 gate |
| P-4 | Three independent teams to run the operations script (D-028) | outside parties | the B7 ship gate |
| P-5 | An independently reviewed threat model for hostile multi-tenant workloads | an outside reviewer | any shared-hosting direction (not the first release) |

## Documents

- `docs/END_GOAL.md` — what done means, scope, invariants, the four
  required failure cases with their drills and pass conditions, the ship
  gate in testable form
- `docs/ARCHITECTURE.md` — sources of truth, components, paths,
  consistency, and the control-plane threat model
- `docs/ROADMAP.md` — B0–B7 with per-milestone proof gates
- `docs/DECISIONS.md` — the guide-seeded trade-offs (D-001–D-010), the
  decisions made before code (D-011–D-028) and the questions still open
- `AGENTS.md` — project rules on top of the workspace rules

Contract: the engineering guide `09_Backend_Platform.docx` (kept in the
maintainer's workspace under `docs/Project_Guides/`, not in this
repository); it wins on conflict.

**What counts as done:** the ship gate in `docs/END_GOAL.md` — three
independent teams run the operations script from the documentation alone,
faster than the same operations assembled by hand — with the four contract
failure cases proven and the load results committed with machine + commit.
