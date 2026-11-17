# B1-T acceptance: Durable Runner adopts the template

`docs/STARTER_TEMPLATE.md` item 11 makes Durable Runner's adoption the template's acceptance
test: on a fresh 2 GB droplet, with nothing but the documentation, install it, back up and
restore each of its stores, upgrade one version and roll back, every step passing its check,
in less total time than the same steps done by hand, with the hand baseline timed first.

**Status: not run.** It needs a droplet, and the hand baseline must be timed before anyone
uses the template (as D-028 requires of the operations script). What exists is a dry run in a
scratch copy, recorded below: the drill that touches Durable Runner's store ran against its
own binaries, and the template's pieces were dropped into a copy of its workspace to see what
adoption changes. Durable Runner itself was not modified.

## The dry run (2026-11-17)

Durable Runner at `9d240a7`, exported with `git archive` into a scratch directory, built there
with its own target directory (release, `--locked`), the directory deleted afterwards. The
development machine was busy (load averages 7 to 67 during the session); no timing here is
a measurement.

| What | Result |
|---|---|
| `template/scripts/drills/journal-restore.sh` against `durable-server`, `durable-cli` and the pilot handler: 40 two-step jobs to done, `/admin/backup`, the copy packed and age-encrypted, the key restored from its backup, `durable-cli restore`, a second server replaying it, SIGTERM | **passed**, 9 steps: every run came back with the same id, state and finish time; the restored server drained and exited 0 |
| `crates/starter-http` copied in as `crates/durable-http`, a workspace member | its 20 tests pass after two mechanical changes: the manifest rewritten for Durable Runner's workspace (it has no `[workspace.dependencies]` for hyper, tower or uuid, and is edition 2021: the crate keeps `edition = "2024"`), and `starter_http` renamed. Clippy with `-D warnings` then refuses the tests' `expect` calls: Durable Runner has no `clippy.toml` allowing them in tests |
| `scripts/check-pins.sh` on Durable Runner's workflow | passes (its actions are SHA-pinned) |
| `scripts/check-results.sh` on its 22 result files | passes (no host name since its 2026-11-17 fix) |
| `scripts/copy-template.sh` (a whole-template copy, as `copytest`, `--verify`) | builds, passes clippy and 70 tests with no template name left; the manifest's digests survive the rename |

## What adoption changes in Durable Runner

| Area | Durable Runner today | With the template | Size |
|---|---|---|---|
| Accept loop | its own `serve_http` with the same bounds (D-045, D-049), connections past the cap waiting in the backlog | keep its own (it is already bounded and tested), or replace it with `durable-http` and gain the shared proofs (`prove-limits.sh`); no behaviour gained either way | small, optional |
| Lints | `unwrap_used`/`expect_used` warn, no `clippy.toml` | add `clippy.toml` with `allow-unwrap-in-tests` and `allow-expect-in-tests`, as the template has | one file |
| Health | `/health`, `/ready` | add `/livez` (alias of `/health`), `/readyz` (alias of `/ready`) and `/errorz` (the server-error window) so Service Monitor watches the same contract on every product | small: the window lives in `durable-http`'s stack |
| Secrets | `DURABLE_API_KEY` and `DURABLE_READ_KEY` from the environment only | read them from `$CREDENTIALS_DIRECTORY` (or a `_FILE`), so the unit can use `LoadCredential=` and the environment file holds no secret | a code change in `api_key_from_env` and `read_key_from_env` |
| Logs | `eprintln!` lines | JSON through `tracing`, with request ids, as the template's stack provides; journald retention from `deploy/journald-starter.conf` | medium |
| Host | none in the repository | `deploy/Caddyfile` (upstream 127.0.0.1:58300), `durable.service` from `starter.service` with `ReadWritePaths=` for the journal volume and the backup root, `RequiresMountsFor=` for the volume, `TimeoutStopSec` above its 30 s drain (its OPERATING.md says 70 s), `deploy/harden.sh` as is | new files |
| Releases | none | `deploy.sh` and `rollback.sh` without the SQL steps: the compatibility gate is the journal's `FORMAT_VERSION`: a binary already refuses a frame version it does not know, though it reports it as corruption (`version N`), where the template's pattern would name it newer; a rollback within one format needs no restore | adapt two scripts |
| Upgrade rehearsal | `examples/pilot/run.sh` covers kill, backup, restore and drain, not an upgrade | a rehearsal on the template's pattern: vN with jobs, vN+1, replay checked, roll back to vN, replay checked again | new script |
| Restore drills | the pilot's restore, `perf/drill.py` | `journal-restore.sh` (passed above) and, from S2, `postgres-restore.sh --neon` for its account records | copy |
| CI | its gate with cargo-deny 0.20.2, SHA-pinned actions, `contents: read` | add `check-pins.sh`, `check-results.sh`, `install-age.sh`; Dependabot groups limited to minor and patch, with a week's cooldown | small |
| Accounts | none: one API key (S1) | the accounts kit and Neon, at its S2 phase: invite-gated sign-up, workspaces, members, per-workspace tokens in place of the single key; every job and handler scoped by workspace, which the kit's isolation tests do not cover (they cover the kit's own routes) | large |

## What the acceptance run still needs

1. A fresh 2 GB droplet (Ubuntu 24.04) and a DNS name.
2. **The hand baseline, timed first**, by someone following only Durable Runner's own
   `OPERATING.md`: install behind TLS, back up and restore the journal, upgrade one version
   and roll back. Times and every check recorded.
3. The adoption changes above that the run depends on (secrets from credentials, the unit,
   the release scripts, the rehearsal), made in Durable Runner by its own process.
4. The same steps with the template's documentation and scripts, timed.
5. The result committed: every step's check, both totals, the machine, the commits. A miss
   is recorded, not re-run until green. Until then the template's benefit is a claim.
