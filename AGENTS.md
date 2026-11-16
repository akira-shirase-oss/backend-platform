# Backend Platform — agent instructions

A self-hosted control plane that makes small applications easier to
*operate*: reconciled provisioning over mature components (PostgreSQL, a
reviewed OIDC integration, S3-compatible storage, standard containers) —
never rewritten infrastructure. Contract:
`../../docs/Project_Guides/09_Backend_Platform.docx` — the guide wins on any
conflict. Status: **planning**; no code exists yet, and the guide marks
this an expansion horizon, not an approved simultaneous build.

## Read first

1. `docs/END_GOAL.md` — what done means, scope, the four required failure
   cases, the ship gate, the open questions.
2. `docs/ARCHITECTURE.md` — sources of truth, components, paths,
   consistency.
3. `docs/ROADMAP.md` — B0–B7; each milestone's proof gate precedes the
   next.
4. `docs/DECISIONS.md` — guide-seeded trade-offs (D-001…).

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
   shared contracts are limited to identity, IDs, events and health; no
   shared mutable database across unrelated products.
5. **Least privilege per project.** Principals, DB roles, storage paths,
   secrets and network boundaries are scoped; admin actions carry
   explicit authority and an audit row.
6. **Bounded everything.** Every operation, queue, body, retry loop and
   budget has a limit and a deadline. No database transaction spans a
   provider call — intent commits to the operation log first.
7. **Cache is disposable.** An accelerator only where evidence justifies
   it; never the sole durable job store, never an authority.
8. **Honest upgrade and exit.** Version manifests, compatibility matrix,
   consistent backups and export; reconcile after restore before
   replaying jobs — no blind replay of remote effects.
9. **Evidence over claims.** `perf/` results carry machine + commit; load
   runs are million-scale where a traffic unit exists (here:
   control-plane operations, app-facing requests, provisioning runs);
   misses are kept, never rerun until green. Laptop numbers are
   development observations.

## Rust rules

- `export PATH="$(brew --prefix rustup)/bin:$PATH"` before every cargo
  command — Homebrew's cargo ignores the workspace pin (Rust 1.99.1).
- `platform-domain` carries no I/O — enforced like the sibling projects'
  domain crates.
- Typed errors (`thiserror`) in libraries; `anyhow` only in binaries.
- No `unwrap`/`expect` on external input. Deadlines on every I/O await.
- No secrets, tokens or app payloads in logs.

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

## Verification

`scripts/check.sh` (once the repo exists) runs what CI runs: toolchain,
fmt, clippy `-D warnings`, tests, the domain architecture check, then the
fixture and load smoke runs when the stack is up. Run it before every
push.

## Working rules

- Work in small slices; each states the user outcome, the data invariant,
  the failure case, the permission and the acceptance test.
- Expand only from observed need — the ship gate is saved effort, not
  surface area. When the guide leaves something open, it is an open
  question in `END_GOAL.md`, not a license to guess.
