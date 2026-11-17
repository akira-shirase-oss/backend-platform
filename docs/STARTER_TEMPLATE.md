# Starter template — milestone B1-T

The B0 memo recommends staying parked and meeting the need with a copyable
starter template (`docs/B0_MEMO.md` §7). That recommendation was accepted
with corrections on 2026-11-16 (D-036). This document is the template's
contract: what it contains, how each piece is proven, and the acceptance
test that turns its benefit from a claim into evidence.

**Status: planned.** Nothing here is built yet. The control-plane crates in
this repository stay parked, and B1–B7 of `ROADMAP.md` do not start.

## The model: copy, then own

- The template lives in this repository under `template/`. A product copies
  it once and then owns its copy, the same model the products use for
  `@colophon/ui`. Its home is here because its CI and evidence kit (item 8)
  is this repository's, already exercised by B0.
- **No product depends on this repository** at build time or run time.
  There is no shared package, no shared service, no shared login and no
  shared database (D-011). Each product's copy runs against that product's
  own database.
- Template changes reach products only by a documented diff procedure
  (item 10), never by an automatic update.

## Contents

Each item names its source, the files it adds to a product, and the test
or drill that proves it. A test fails without the thing it proves; drills
record their time.

### 1. Accounts kit (the slice D-012 names)

The work all three `SAAS_PLAN.md` files repeat: invite-gated sign-up, email
verification and password reset, sessions, workspaces, members and roles,
per-workspace API tokens, plan limits, and account and workspace deletion.

- **Form:** a Rust module (`accounts/`) with its own migrations. The
  product copies it into its workspace and owns it: its tables live in the
  product's database, and its routes mount in the product's router.
- **Source:** Colophon's auth module (`docs/modules/03-auth.md`,
  `crates/colophon-auth`), with Colophon-specific concepts taken out.
- **Pieces:**
  - email through an SMTP adapter port (one trait; the product picks
    Postmark, Resend or SES);
  - tokens hashed at rest, identifiable by prefix, scoped to one workspace,
    and shown once;
  - plan limits checked at admission, not in a dashboard;
  - deletion that removes or anonymizes every row the kit owns.
- **Identity across products:** only an OIDC *contract* (token format,
  claims). Never a shared identity provider.
- **Tests that ship with it:**
  - cross-workspace isolation negative tests (workspace A's token, session
    and member are refused on workspace B's every route);
  - a property test for plan limits at their edges;
  - a log-capture test that fails if a token, password, reset link or
    session id appears in logs;
  - the invite and verification flows end to end against a fake SMTP sink.

### 2. Host layer

- A **Caddyfile**:
  - TLS and HSTS;
  - request body and timeout limits;
  - the upstream only on 127.0.0.1;
  - health routes passed through.
- A **hardened systemd unit**, based on Service Monitor's
  `deploy/monitor-server.service`:
  - a dedicated user that owns only its data directories;
  - `ProtectSystem=strict`, `NoNewPrivileges=yes`, an empty
    `CapabilityBoundingSet`;
  - `MemoryMax` sized for a 2 GB droplet;
  - `LoadCredential=` for secrets;
  - restart with backoff.
- **Droplet hardening steps**, as a script with a check mode: SSH keys
  only, a firewall that allows 22/80/443, unattended security upgrades.
- **Proof:** a `systemd-analyze security` score is recorded per unit, and
  the check mode fails on a droplet missing any step.

### 3. Configuration and secrets

- An environment template listing every variable with its default and
  bounds.
- Secrets as 0600 files or systemd credentials, never in the environment
  file the unit prints.
- Typed configuration validated at startup, reporting every problem at
  once (the pattern of this repository's `config.rs`).
- **Proof:** the log-capture test pattern, run over startup, every drill,
  and the accounts kit's flows.

### 4. Releases

- Service Monitor's release layout: `releases/<release>/` plus a `current`
  symlink (`scripts/deploy.sh`), with deploy and rollback scripts.
- Forward-only migrations, plus a binary/schema compatibility check: a
  binary refuses to start against a schema newer than it knows, and the
  deploy script refuses to activate a release whose minimum schema is
  missing.
- **Proof:** a test that a too-new schema is refused, and the rollback
  drill (item 7).

### 5. Health contract

- `/livez` (the process answers) and `/readyz`, with a deadline on each
  dependency check.
- The degraded-not-fabricated rule: an unreachable dependency is reported
  as such, never as green.
- Service Monitor watches each product from outside, through its public
  API, as a customer would.
- **Proof:** `/readyz` answers 503 within its deadline when the database is
  unreachable (this repository's `a_dead_control_database_is_reported_honestly`
  pattern).

### 6. Backup and restore drills for the real targets

One drill per data store the products actually use. Each drill is scripted,
has a pass condition (row counts and checksums before and after) and
records its time.

| Store | Product | Drill |
|---|---|---|
| Neon Postgres | Colophon; the account records of all three | point-in-time restore to a branch, verify, promote. Not `pg_basebackup`, which cannot run against Neon |
| SQLite (`VACUUM INTO`) | Service Monitor | snapshot, restore into a fresh directory, verify |
| append-only journal | Durable Runner | `/admin/backup` snapshot, restore, replay check |
| Spaces or R2 bucket | Colophon media | export, then restore into an empty bucket |

Off-droplet copies are encrypted with `age`. The drill includes restoring
the key backup, because losing the key loses every copy.

### 7. Upgrade rehearsal

A script that installs vN with data, upgrades to vN+1, verifies, rolls back
to vN, and verifies again. Each step has a pass or fail. Colophon's
`scripts/upgrade-rehearsal.sh` is the starting point.

### 8. The CI and evidence kit (from this repository, with the review's fixes)

- the pinned toolchain and `scripts/check-toolchain.sh`;
- `scripts/check.sh` as the one gate CI runs;
- `deny.toml`, with a daily advisory run;
- SHA-pinned actions and `scripts/check-pins.sh`, both of its failure
  proofs included;
- the architecture-check pattern, reading the resolved graph for every
  feature and target, with its patched-manifest proof
  (`scripts/test-architecture-check.sh`);
- the checksum-verified k6 installer;
- `load.sh`, recording machine, commit and load average and keeping
  misses.

### 9. Observability minimum

- JSON logs to journald, with bounded retention.
- Request ids on every request and log line.
- An error-rate alert, delivered through Service Monitor.
- No tracing backend until a measured need exists.

### 10. Upkeep

- A component manifest: every component with its version, an owner and an
  advisory watch (the guide's "budget maintenance before adding adapters").
- A documented "pull template changes" procedure, a reviewed diff per
  product, because every product owns its copy.

### 11. Acceptance

**Durable Runner adopts the template.** It is the product with none of
this today. On a fresh 2 GB droplet, with nothing but the documentation:

1. install;
2. back up and restore each of its stores;
3. upgrade one version and roll back.

**Pass:** every step passes its check, and the total time is lower than
the same steps done by hand. The hand baseline is timed **before** the
template is used, as in D-028. A miss is recorded, not re-run until green.
Until this run exists, the template's benefit is a claim.

## Order of work

1. Items 8, 3 and 5: the gate, the configuration and the health contract.
   These are cheap and already proven here.
2. Items 2, 4 and 7: the host, releases and upgrade rehearsal. Durable
   Runner is the first target.
3. Item 6: the restore drills for Durable Runner's journal and its Neon
   account records.
4. Item 11: the timed hand baseline, then the acceptance run.
5. Item 1, the accounts kit, from Colophon module 03. It is the largest
   piece. It follows once the template has a product to land in, and
   Durable Runner and Service Monitor both need it for their S1 phases.
6. Items 9 and 10.

## Out of scope

- A shared login, identity provider, account service or database. D-011
  forbids these; the accounts kit is copied code, not a service.
- Provisioning on demand. That is the parked control plane; the memo lists
  what would bring it back.
- Hosted or multi-tenant operation of the template itself.
