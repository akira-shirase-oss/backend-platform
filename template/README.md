# Starter template

A small Rust service that a product copies once and then owns: a bounded HTTP layer,
PostgreSQL with an owner/runtime role split and a protections manifest, a copy-and-own
accounts kit, the health contract, the host layer, release and restore scripts, and the CI
and evidence kit. It is milestone B1-T of the Backend Platform: that repository's
`docs/STARTER_TEMPLATE.md` is the contract and its D-036 the decision. A copy depends on
nothing: no shared package, service, login or database.

It is an HTTP API with no web interface. A product's dashboard and sign-in pages are its own,
built on its copy of Colophon's UI kit.

## Quickstart

You need Docker, `rustup` (the toolchain installs itself from `rust-toolchain.toml`),
`curl`, `jq` and `python3`. The full gate also needs `sqlite3`, and `age`, which
`scripts/install-age.sh target/tools/bin` installs pinned and checked.

<!-- quickstart:begin -->
```sh
scripts/stack.sh up                        # PostgreSQL 18 in Docker, on 127.0.0.1:61433
scripts/dev.sh start                       # build, migrate, run the service on 127.0.0.1:61450
curl -fsS http://127.0.0.1:61450/readyz    # {"database":"ok","status":"ready"}
invite=$(scripts/dev.sh invite)            # sign-up is invite-gated
curl -fsS -H 'content-type: application/json' \
  -d "{\"invite\":\"$invite\",\"email\":\"you@example.test\",\"password\":\"a long dev password\"}" \
  http://127.0.0.1:61450/v1/signup         # {"user_id":"…","verification_email":"sent"}
scripts/dev.sh mail                        # the verification link the service just mailed
scripts/dev.sh stop
scripts/check.sh                           # the whole gate, as CI runs it (a few minutes)
```
<!-- quickstart:end -->

CI runs this block as written (`scripts/test-readme.sh`), on a fresh runner. Every script
answers `--help`. To use a PostgreSQL 18 you already administer instead of Docker's, export
`STARTER_TEST_DATABASE_URL` (an administrator's URL) first; `scripts/stack.sh up` then
starts nothing.

Next: `docs/ACCOUNTS.md` for the API, `docs/OPERATIONS.md` for a droplet, and
`docs/COPYING.md` before you copy it.

## What is here

| Path | What | Proven by |
|---|---|---|
| `crates/starter-http` | the accept loop (connection cap before accept, header-read and idle timeout, write-stall deadline, requests per connection, lifetime, drain deadline), the middleware stack (request ids, one error envelope, panics as 500s, request deadline, body limits), load shedding, the server-error window, client keys, bounded token buckets | `tests/limits.rs` over raw sockets |
| `crates/starter-db` | the bounded pool, the owner/runtime role split, the startup check with the protections manifest (`sql/protections.sql`), binary/schema compatibility, fingerprints, test support, the log-capture check | `tests/protections.rs` (13 weakenings, each alone, refused), `schema.rs`, `fingerprint.rs`, `logcheck.rs` (the planted-leak controls) |
| `crates/starter-accounts` | invite-gated sign-up, email verification and reset through an SMTP port, sessions, sign-in budgets per address and global with a trusted path floods cannot block, bounded password work, workspaces, members and roles, API tokens, plan limits at admission, deletion (`docs/ACCOUNTS.md`) | `tests/flows.rs` (a fake SMTP sink), `isolation.rs`, `limits.rs`, `budgets.rs`, `deletion.rs`, `logs.rs` |
| `crates/starter-server` | typed configuration (secrets from systemd credentials or owner-only files, every problem at once), `/livez`, `/readyz`, `/errorz`, the wiring, `starterctl` | `tests/binary.rs` (the real binary: drains on SIGTERM, refuses the owner role, a foreign schema, weakened protections and a bad configuration), `tests/health.rs` |
| `migrations/` | `0001_base.sql` (every copy), `0002_accounts.sql` (the kit) | the manifest check on every test database |
| `deploy/` | `Caddyfile` and `caddy.service`, the hardened `starter.service`, journald retention, `env.example`, `harden.sh` (apply and check), the development `compose.yaml` | `scripts/check-caddy.sh`, `scripts/check-unit.sh` (exposure 1.4), `scripts/test-harden-check.sh`; `scripts/test-install.sh` runs `docs/OPERATIONS.md`'s install, check, upgrade and rollback blocks as written on Ubuntu 24.04 with systemd (CI) |
| `scripts/deploy.sh`, `rollback.sh` | the release layout, the schema gate on activation and on rollback | `scripts/upgrade-rehearsal.sh` (23 steps) |
| `scripts/drills/` | restore drills: PostgreSQL, Neon, SQLite, an append-only journal, a bucket; off-droplet copies with age and the key restored from its backup | `test-drills.sh` (each fails on a weakened source and a wrong key); the Neon and bucket drills are not run yet (no credentials) |
| `scripts/` (the rest) | the gate (`check.sh`), development (`stack.sh`, `dev.sh`), toolchain, pin, results and architecture checks with their failure proofs, `prove-limits.sh`, `load.sh` and `perf/scenarios/service.js`, pinned installers for k6 and age | the gate; `test-help.sh`; `test-readme.sh` |
| `.github/` | the copy's CI (SHA-pinned actions, `contents: read`, cargo-deny 0.20.2, daily) and Dependabot (grouped minor and patch, a week's cooldown) | `scripts/check-pins.sh` |

Each of the 16 limits is proven by a test that fails when the limit is removed:
`scripts/prove-limits.sh` removes each in turn (the lines marked `// LIMIT(name)`).

## Documents

- `docs/COPYING.md`: copying it (`scripts/copy-template.sh` in the Backend Platform
  repository), what must stay in step, pulling later template changes.
- `docs/OPERATIONS.md`: the droplet, configuration and secrets, releases, upgrade and
  rollback, backup and restore, health, observability, load.
- `docs/ACCOUNTS.md`: the accounts kit's routes, secrets, budgets, limits and deletion.
- `docs/COMPONENTS.md`: every component's version, where it is pinned, who owns it, how
  advisories arrive.
