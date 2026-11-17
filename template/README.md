# Starter template

A small Rust service skeleton that a product copies once and then owns: the bounded HTTP
layer, PostgreSQL with an owner/runtime role split and a protections manifest, a
copy-and-own accounts kit, the health contract, the host layer, and the CI and evidence
kit. It is milestone B1-T of the Backend Platform (its `docs/STARTER_TEMPLATE.md` is the
contract and D-036 the decision). Nothing depends on the repository it came from: no
shared package, service, login or database.

**Status:** being built. What exists is listed below with the test that proves it; what
does not is listed as owed. `docs/COPYING.md` says how to copy it and what to keep in step.

## What is here

| Path | What | Proven by |
|---|---|---|
| `crates/starter-http` | the accept loop (connection cap before accept, header-read and idle timeout, write-stall deadline, requests per connection, lifetime, drain deadline), the middleware stack (request ids, one error envelope, panics as 500s, request deadline, body limits), load shedding, the server-error window, client keys, bounded token buckets | `tests/limits.rs` over raw sockets; each limit removed in turn by `scripts/prove-limits.sh` makes its test fail |
| `crates/starter-db` | the bounded pool, the owner/runtime role split, the startup check with the protections manifest (`sql/protections.sql`: triggers, function bodies, search paths, exact grants, ownership, no `TEMP`/`CREATE`), binary/schema compatibility, fingerprints, test support, the log-capture check | `tests/protections.rs` (13 weakenings, each alone, refused), `tests/schema.rs`, `tests/fingerprint.rs`, `tests/logcheck.rs` (the planted-leak controls) |
| `crates/starter-accounts` | invite-gated sign-up, email verification and reset through an SMTP port, sessions, sign-in budgets per address and global with a trusted path floods cannot block, bounded password work, workspaces, members and roles, per-workspace API tokens, plan limits at admission, account and workspace deletion | `tests/flows.rs` (against a fake SMTP sink), `isolation.rs`, `limits.rs`, `budgets.rs`, `deletion.rs`, `logs.rs` |
| `crates/starter-server` | typed configuration (secrets from systemd credentials or owner-only files, every problem reported at once), `/livez`, `/readyz` (a deadline on the database probe), `/errorz`, the wiring, `starterctl` | `tests/binary.rs` (the real binary: drains on SIGTERM, refuses the owner role, a foreign schema and a bad configuration), `tests/health.rs` |
| `migrations/` | `0001_base.sql` (what every copy keeps), `0002_accounts.sql` (the kit) | the manifest check on every test database |
| `deploy/` | `Caddyfile`, the hardened `starter.service`, journald retention, `env.example`, `harden.sh` (apply and check), the development `compose.yaml` | `scripts/check-caddy.sh`, `scripts/check-unit.sh` (exposure 1.4), `scripts/test-harden-check.sh` |
| `scripts/` | `check.sh` (the gate), toolchain, pin, results and architecture checks with their failure proofs, `prove-limits.sh`, `stack.sh`, pinned installers for k6 and age | the gate runs them all |
| `.github/` | the copy's CI (SHA-pinned actions, `contents: read`, cargo-deny 0.20.2, daily runs) and Dependabot (grouped minor and patch, a week's cooldown) | `scripts/check-pins.sh` |

## Commands

```
scripts/stack.sh up          # PostgreSQL 18 on 127.0.0.1:61433 (compose project starter-template)
eval "$(scripts/stack.sh env)"
scripts/check.sh             # the gate: what CI runs
scripts/prove-limits.sh      # every limit's test fails without the limit
```

Use the pinned toolchain through rustup (`rust-toolchain.toml`, Rust 1.99.1);
`scripts/check-toolchain.sh` fails on any other `rustc`.
