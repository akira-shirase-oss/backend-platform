# Backend Platform — agent instructions

A self-hosted control plane that makes small applications easier to
*operate*: reconciled provisioning over mature components (PostgreSQL, a
reviewed OIDC integration, S3-compatible storage, standard containers) —
never rewritten infrastructure. Contract: the engineering guide
`09_Backend_Platform.docx` (kept in the maintainer's workspace under
`docs/Project_Guides/`, not in this repository) — the guide wins on any
conflict. Status: **parked (future horizon); B0 in progress** at the maintainer's
request (2026-11-16): built and green, with a quiet-machine load baseline
and the 1,000,000-request run still owed. The memo (`docs/B0_MEMO.md`)
recommends staying parked; it was accepted with corrections (D-036). The
next milestone is B1-T, the copy-and-own starter template
(`docs/STARTER_TEMPLATE.md`). The control-plane milestones B1–B7 do not
start until the maintainer unparks the project.

## Read first

1. `docs/END_GOAL.md` — what done means, scope, the four required failure
   cases with their drills and pass conditions, the ship gate.
2. `docs/ARCHITECTURE.md` — sources of truth, components, paths,
   consistency, the control-plane threat model (T1–T10).
3. `docs/ROADMAP.md` — B0–B7; each milestone's proof gate precedes the
   next; a status cell says *Done* only with named tests and evidence.
4. `docs/DECISIONS.md` — trade-offs and decisions (D-001 onward; read
   them all, the B0 ones included) and the questions still open.

## Non-negotiables

1. **Desired ≠ observed.** The control DB stores desired configuration;
   actual resources report observed state. Status is derived;
   `partial`, `uncertain` and `unknown` are first-class, never hidden.
2. **Reconciled provisioning.** Operations carry IDs and recorded steps;
   retries resume the same operation and re-check observed state — never
   a blind retry that duplicates a database.
3. **Running apps outlive the control plane.** A dead control UI or plane
   must not stop applications; management shows degraded, never
   fabricated success.
4. **No dependency trap.** Products run independently before integration;
   shared *contracts* (OIDC tokens, ID formats, events, health shape) are
   allowed, a shared login service or mutable database is not. This
   platform is never the login for the existing independent products
   (D-011).
5. **Least privilege per project, none in the API.** Principals, DB roles,
   storage paths, secrets and network boundaries are scoped; admin actions
   carry explicit authority and an append-only audit row. Provider admin
   credentials live only in the worker; `platform-server` never links
   `platform-adapters` (D-013).
6. **Bounded everything.** Every operation, queue, body, retry loop and
   budget has a limit and a deadline. No database transaction spans a
   provider call — intent commits to the operation log first.
7. **Cache is disposable.** An accelerator only where evidence justifies
   it; never the sole durable job store, never an authority.
8. **Honest upgrade and exit.** Version manifests, compatibility matrix,
   consistent backups and export; reconcile after restore before
   replaying jobs — no blind replay of remote effects.
9. **Evidence over claims.** `perf/` results carry machine + commit; the
   1,000,000-unit runs count control-API requests against the fake
   providers and app-facing requests across the ten starter projects
   (D-018); real providers get a separate, small count that is never
   extrapolated, and no hosted third party is load-tested. Misses are kept,
   never rerun until green. Laptop numbers are development observations.

## Rust rules

- `export PATH="$(brew --prefix rustup)/bin:$PATH"; unset CARGO_TARGET_DIR`
  before every cargo command — Homebrew's cargo ignores the workspace pin
  (Rust 1.99.1, `rust-toolchain.toml`; `scripts/check-toolchain.sh` fails on
  any other rustc).
- `platform-domain` carries no I/O — enforced like the sibling projects'
  domain crates.
- Typed errors (`thiserror`) in libraries; `anyhow` only in binaries.
- No `unwrap`/`expect` on external input. Deadlines on every I/O await.
- No secrets, tokens or app payloads in logs — the log-capture test
  (D-015) runs over every drill.

## Frontend rules

- SvelteKit + TypeScript (strict); the console is a static SPA served by
  the platform server on the same origin. It holds no authority and never
  queries the control database.
- The console tells the truth about the control plane: unreachable or
  degraded management renders as such, never a green empty board.

## Testing rule (shared workspace standard)

Every feature ships regression tests + a fault-fixture path + a load
scenario, in the same change. Every fixed bug gets a test that fails
without it. The four contract failure cases are drills, not aspirations —
and the partial-provisioning drill is practiced before any second adapter
is added.

## Verification (CI from the first B0 commit)

`scripts/check.sh` runs what CI runs:

- toolchain, fmt, clippy `-D warnings` (also on the fault-injection build);
- the architecture checks: `platform-domain` has no I/O; `platform-server`
  links no adapter, worker or fake provider and reads no `BP_PROVIDER*`
  setting; once the console exists, it imports no database client;
- the pin check (`scripts/check-pins.sh`, T8);
- tests against PostgreSQL;
- drill `partial-provision` (in process, plus the `kill_drill` test on the
  `fault-injection` build);
- a short load probe;
- `cargo deny`.

Each check is shown able to fail. Run it before every push; check the exit
code and never pipe it through `grep`.

Local stack: `scripts/stack.sh up` (compose project `backend-platform`,
PostgreSQL on 127.0.0.1:61432; the platform's own processes use
61400–61499: server 61400, fake provider 61410). Load runs:
`MODE=smoke|full scripts/load.sh`. A full run is taken only when `uptime`
shows a load average below about 6; every result records the machine, the
commit and the load.

## Working rules

- Work in small slices; each states the user outcome, the data invariant,
  the failure case, the permission and the acceptance test.
- Expand only from observed need — the ship gate is saved effort, not
  surface area. When the guide leaves something open, it is an open
  question in `docs/DECISIONS.md` or a new D-NNN, not a license to guess.
