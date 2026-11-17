# B0 memo — a starter template or a running control plane?

Written 2026-11-16 for the maintainer's decision (README P-2, D-012). It
answers the identity question first (D-011), describes what one small
organization actually needs to run its small apps, measured against the
portfolio's own products, and then compares the ways to meet that need. It
ends with a recommendation, marked as such, and the conditions that would
change it.

Facts are dated. Component versions were read from each project's official
source on 2026-11-16 (§6). Measurements come from this repository's B0 spine
and carry the commit and machine load (§5). Anything not measured says so.

**Decision (2026-11-16):** the recommendation was accepted with corrections
by the coordinator, acting on the maintainer's delegation (D-036). An
independent review corrected this memo in seven places: the accounts
slice, deployed versus planned, the operations-line count, the smoke
figures, the steelman, the threat count, and the drill's "0 duplicates"
claim (§5). Those corrections are made below. The next
milestone is the starter template, B1-T (`docs/STARTER_TEMPLATE.md`).

## 1. The identity question, answered first

The work the products repeat most is accounts. Each product's
`docs/SAAS_PLAN.md` (written 2026-11-13) lists the same gap rows between
"my tool" and "SaaS": accounts, invite-gated sign-up, email verification,
workspaces, members and roles, per-workspace tokens or API keys, plan
limits, onboarding and account lifecycle. That slice is the recurring
developer problem D-012 names.

**D-011 forbids a shared login *service*, not shared *code*.** A platform
that ran the products' identity would make all three depend on it: the
guide's dependency trap. But a copy-and-own accounts kit does not. Each
product copies the code once, runs it against its own database, and owns
it from then on, as it already owns its copy of `@colophon/ui`. So the
starter-template option (a) includes an accounts kit (§3). The slice the
products repeat most is addressed by the template, and not by a platform.

The platform's own identity offer is one OIDC realm per project, and it
has no consumer among the reference workloads:

- Colophon keeps Keycloak off its planned droplet for memory reasons and
  plans password accounts only for the beta (Colophon `SAAS_PLAN.md`,
  "Keycloak");
- Service Monitor and Durable Runner plan their own accounts on Neon.

## 2. What the reference workloads actually need

The three products stay independent (D-011). They are measured here as
workloads, not proposed as tenants. **None of them is deployed yet.** All
three plans say "plan, not built … Nothing here is deployed yet". The host
column below is the 2026-11-13 decision, not something observed: no
product has a Caddyfile, Service Monitor has the only systemd unit, and
Colophon's planned `deploy/droplet/` does not exist.

| | Colophon | Service Monitor | Durable Runner |
|---|---|---|---|
| Process | one Rust binary (API + Studio + worker loop) | one Rust binary + static status page | one Rust binary (API + dashboard + engine) |
| Host (planned) | one 2 GB droplet, Caddy → systemd | its own 2 GB droplet (observes the others from outside) | one 2 GB droplet, Caddy → systemd |
| Durable data | Neon Postgres (LISTEN/NOTIFY, advisory locks, `SKIP LOCKED`) | SQLite on the droplet (evidence) + Neon (accounts) | append-only journal on a volume (the authority) + Neon (accounts) |
| Files | S3 API: Spaces or R2 planned; Garage only in the dev stack | none | none |
| Cache | Valkey (capped) for quotas | none | none |
| Jobs | in-process (scheduled publishing stays inside, D-060) | the probe loop, in-process | it *is* the job runner |
| Backups today | `pg_basebackup`/`pg_verifybackup` (cannot run on Neon; PITR planned) | `VACUUM INTO` + `backup-drill.sh` | `/admin/backup` + restore drill |

Sources: each product's `docs/SAAS_PLAN.md`; `products/colophon/deploy/compose.yaml`;
`products/service-monitor/deploy/`.

Operations material that exists today, counted on 2026-11-16:

- Colophon:
  - scripts: `backup.sh` (719 lines), `restore.sh` (733),
    `upgrade-rehearsal.sh` (178), `install.sh` (154), `failover.sh` (110),
    `backup-crypt.sh` (74);
  - runbooks: backup, restore, upgrade, observability, install (594 lines).

  About 1,500 of these lines (`backup.sh`, `restore.sh`, `backup-crypt.sh`)
  are base-backup, WAL and `age` logic built on `pg_basebackup`, which
  Colophon's own plan says "cannot run against Neon".
- Service Monitor: `backup-drill.sh` (85), `deploy.sh` (80), `RUNBOOK.md`
  (162), and a hardened `monitor-server.service` (60).
- Durable Runner: `docs/OPERATING.md` (249 lines) and its backup and
  restore endpoints.

That is about 3,200 lines of operations material. Much of it is not
duplication: it is product-specific, or aimed at a database the plans have
moved away from. The slice that would repeat is still mostly unwritten:
proxy, service unit, environment and secrets file, deploy and rollback,
health contract, and restore drills for Neon, SQLite and the journal. The
template would standardize those parts before three products write them
three times.

None of the products needs a database, a bucket or a realm *provisioned on
demand*. Each needs the same operating chores done well once:
- TLS and a reverse proxy;
- a hardened service unit;
- an environment and secrets file;
- backups with a restore drill;
- an upgrade procedure with a rehearsal;
- health and readiness endpoints;
- logs.

Plus the accounts slice of §1. The planned managed services (Neon; Spaces
or R2) provide the durable data and files.

## 3. The options

**(a) A copyable starter template.** A directory that each product copies
and then owns. It contains:
- **a copy-and-own accounts kit:** sign-up, verification, sessions,
  workspaces, roles, tokens and limits, running on the product's own
  database;
- a Caddyfile;
- hardened systemd units;
- an environment template with secret-file conventions (mode 0600, never
  in logs);
- backup, restore and upgrade scripts, each with its drill;
- a health and readiness contract;
- the CI and load harness this repository now has.

Nothing runs that the product does not already run. Its full contents are
`docs/STARTER_TEMPLATE.md`.

**(b) A running control plane.** What `ARCHITECTURE.md` describes: a
control database, an API, a reconciler worker holding provider admin
credentials, adapters for PostgreSQL, the object store, the OIDC provider
and the container runtime, a console, a CLI, an OTLP collector and a
diagnostics database. It is assembled from proven components, but someone
has to run it.

**(c) A run-to-completion reconciler CLI.** The steelman of (b). It is B0's
spine without the daemon: `platform apply plan.toml` takes a lock, runs
the plan's steps through the adapters, reconciles, records the outcome and
exits. There is no API, no console and no long-running process, so T1 and
T10 do not exist. The credentials live where the operator runs it (laptop
or CI), exactly as today. It reuses everything B0 built, including the
partial-provisioning drill. What it lacks against (b): continuous drift
detection and a status view between runs. What it lacks against (a): it
still needs real adapters (B2) and their upkeep. It also provisions
resources the products do not currently need provisioned.

**(d) Adopt an existing tool.** Kamal (template shape: deploy scripts over
SSH and Docker, no server-side control plane) or Coolify (platform shape: a
self-hosted PaaS). Either would cover deploy and restart, without the
accounts kit, the restore drills for Neon, SQLite and the journal, or the
evidence kit. The products are single Rust binaries under systemd, and
neither tool covers that natively; both are built around containers.
Kamal has had no release since 2026-06-18, so adopting it would mean
owning its upkeep. Coolify (v4.7.5, 2026-11-10) is itself a control plane
to run (T1, T2, T10). Either one also adds a container runtime to every
droplet, which the products do not have today.

## 4. Comparison

### Reliability

| | Starter template | Control plane |
|---|---|---|
| New runtime components | none | control DB, API, worker, console; plus Keycloak, an object store, Podman and an OTel Collector if self-hosted |
| In the app's request path | nothing new | nothing by design (invariant 3); proving it is the `control-plane-down` drill, owed at B3 |
| Provisioning failure modes | none: resources are created once, by hand or by the provider's console | partial provisioning, lost replies, crashed or paused workers, late creates. B0 drills them against the **fake** provider only; the real adapters are B2 |
| Overload behaviour | each product's own | the control API sheds with 503/429 instead of hanging (seen in the B0 smoke, §5); still one more thing to size |

### Upgrade and restore drills

- **Template:** one upgrade drill and one restore drill per product, for that
  product's own data. Some exist: Colophon's upgrade rehearsal and its
  restore script (aimed at self-hosted PostgreSQL, not Neon), Service
  Monitor's backup drill, Durable Runner's restore drill. The template
  standardizes them and adds the missing ones (§2).
- **Control plane:** every product's drills stay, **and** the platform's own
  are added. Failure case 4 (D-027) has four interruption points, and a
  restore must reconcile observed state before any job resumes. That is more
  drills, not fewer, for as long as the platform holds no app data itself
  (and it must not).

### Security surface (threat model T1–T10, `ARCHITECTURE.md`)

| # | Threat | Template (a) | Reconciler CLI (c) | Control plane (b) |
|---|---|---|---|---|
| T1 | compromised console/API reaches every project | does not exist: no central API | does not exist | exists; mitigated by the API/worker split (CI-checked, all features and targets) |
| T2 | compromised holder of admin credentials | as a process, does not exist. The admin power **moves to the operator's credentials, as today**: the DigitalOcean, Neon and Cloudflare tokens on a laptop or in CI already hold admin over every product | the same operator credentials, now also used by the CLI | exists as a process (the worker): the crown jewel, in addition to the operator's |
| T3 | cross-project access | separate droplets, Neon projects and buckets, but still a provider and product concern (a shared token or a mis-scoped key crosses projects) | the same | must be enforced by roles, keys, realms and networks on a shared host |
| T4 | container escape | no shared container host | none (no runtime) | rootless Podman on a shared host |
| T5 | admin abuse, repudiation | provider audit logs (DigitalOcean, Neon), still to be watched | the same, plus the CLI's own records | an append-only audit to build and protect (D-034's roles are the start) |
| T6 | secret disclosure | per-droplet secret files, blast radius of one product | per-run secrets, operator-held | envelope encryption and a KEK whose loss loses every secret |
| T7 | SSRF via user URLs | a product concern (Service Monitor already guards its own egress) | none | an egress guard to build |
| T8 | supply chain | Caddy and the product | plus the adapters' clients | plus PostgreSQL server, Keycloak (JVM), object store, Podman and OTel Collector |
| T9 | backup theft, malicious restore | per product | per product | central backups: one prize, one restore path for everything |
| T10 | control-plane DoS | does not exist | does not exist | exists |

The template **eliminates T1, T2 as a process, T4 and T10**. T3, T5 and T7
persist, as provider and product concerns. T6, T8 and T9 persist per
product, each with a one-product blast radius. The reconciler CLI
eliminates T1, T4 (no runtime) and T10. The control plane carries all ten,
plus the operator's credentials.

### Operating effort

Neither option's effort has been timed. The honest measure is the D-028
baseline (the operations script done by hand), and nobody has run it yet.
What can be counted:

- **Template:** about 3,200 lines of operations material exist (§2), but
  most of what would repeat is not yet written. The template writes it once
  for every product to copy, and adds no service to patch. The accounts kit
  is the largest piece; it comes from Colophon's module 03.
- **Control plane:** B0's spine alone, after the review round, is about
  6,400 lines of Rust (unit tests included) plus about 2,650 lines of
  integration tests and drills (counted 2026-11-16), and it provisions
  nothing real yet. Running it adds the
  upkeep of every adapter. The guide says to budget an adapter's maintenance
  before adding it: an owner, a pinned version and an advisory watch. The
  OTel Collector is still 0.x, with a release about every two weeks;
  Keycloak is a JVM service with its own advisories to track.
- **Memory:** each product is planned for a 2 GB droplet. The two products
  in this space publish self-hosting minimums above that before any app
  runs:
  - Supabase: 4 GB RAM, 2 cores, 40 GB SSD.
  - Appwrite: 4 GB RAM, 2 cores, 2 GB swap.

  Colophon's plan already leaves Keycloak off its droplet for memory. The
  control plane's idle footprint is unmeasured (a B2 gate).

### Proven components each would use (verified 2026-11-16, §6)

- **Template:**
  - Caddy 2.11.9 and systemd.
  - The products' managed services (Neon; Spaces or R2).
  - For self-hosters: PostgreSQL 18.7 and Garage v2.5.0.
  - k6 for load.
  - For backups: Neon's point-in-time restore and branches (`pg_basebackup`
    cannot run against Neon), SQLite's `VACUUM INTO`, the journal's own
    snapshot, and `pg_dump` for self-hosters.
- **Control plane:** everything the template uses, plus:
  - PostgreSQL 18.7 as the control DB and the provisioned databases.
  - Keycloak 26.8.2.
  - Garage v2.5.0 or SeaweedFS 4.54. MinIO's community edition is excluded:
    its repository says it is no longer maintained, and it was archived on
    2026-04-25.
  - Podman v6.2.0 with Quadlet.
  - OpenTelemetry Collector v0.165.0.

## 5. What B0's spine measured

The spine was built regardless of the verdict, because CI comes first. It
shows that the platform's hardest property is achievable, and at what cost.

- **Drill `partial-provision`, in process**
  (`crates/platform-worker/tests/partial_provision.rs`). 10,000 seeded
  fault schedules in each of three provider modes, 30,000 in all. Each
  schedule mixes:
  - failures before any effect;
  - lost create replies;
  - calls that hang past the deadline after taking effect;
  - **creates that land after the deadline** (`LateEffect`, added after
    the review);
  - simulated worker crashes at every fault point;
  - pre-existing resources.

  | Provider mode | Faults (late creates) | Crashes | Partial rounds retried | Outcome |
  |---|---|---|---|---|
  | **unique names** (like `CREATE DATABASE`) | 61,008 (2,947) | 22,741 | 15,797 | 0 duplicates, 0 orphans; 1,388 `AlreadyExists` answers, resolved by lookup and adoption |
  | **idempotency keys** | 61,076 (2,951) | 22,761 | 15,913 | 0 duplicates, 0 orphans; 1,386 late creates replayed by key |
  | **plain** (neither) | 56,368 (2,710) | 21,975 | 14,808 | 1,387 duplicates made by late creates, **every one reported** (step `blocked: duplicate`, operation `partial`); 0 silent, 0 orphans |

  **Scope:** the plain row is the honest limit. A provider without unique
  names or idempotency keys can be made to hold a duplicate by a create
  that lands late, and no reconciler can prevent that. The platform's duty
  there is to see it and report it (D-035), never to call it a success. The
  first version of this memo claimed "0 duplicates" against a fake that
  never delivered a create late, with workers that were killed but never
  paused. The review showed both gaps; both are now drilled:
  - a paused worker checks its lease before every provider call (D-029);
  - creates carry an idempotency key;
  - every create, and every adoption after an unknown outcome, is verified
    by name.

  A blind-retry reconciler is still caught by the same oracle.
- **Drill `partial-provision`, process level**
  (`crates/platform-worker/tests/kill_drill.rs`, `pause_drill.rs`,
  `concurrent_drill.rs`, all on PostgreSQL):
  - **Kills:** the real worker binary was killed with SIGKILL (signal 9
    asserted) at each step of the starter plan: after the attempt is
    committed, after the lookup, and after the create took effect but
    before it was recorded, plus once after the finish. That is 16 points,
    run against both plain and unique-name providers: 32 cases.
  - **Provider faults:** every fault against every resource kind in both
    modes, plus a late create per kind: 50 cases.
  - **A late create over HTTP** on a plain provider ends visibly `partial`,
    never `succeeded`.
  - **Pauses:** a worker paused past its lease does not call the provider;
    one paused after its lease check has its stale create de-duplicated by
    key.
  - **Concurrency:** three workers of four tasks each, on 40 projects with
    three operations each, with provider latency and 8% faults: every
    operation succeeded, the provider held exactly the plan, 0 duplicates.

- **Control-API load smoke** (`perf/results/`):
  - **Scenario:** 100,000 requests, 80% reads and 20% writes, offered at
    2,000 requests/s for 50 s against the fake provider with 0.1% faults.
  - **Machine:** an Apple M2 Pro laptop (12 cores, 32 GB) shared with other
    builds, at load average **25–65**, well above the quiet-machine bar of
    about 6. Both runs are busy-machine observations, not capacity.
  - **Result:** both runs **failed, and are kept**. Figures are
    scenario-only (`http_req_failed{scenario:control_api}`; setup requests
    excluded), with successes counted over the 50 s window:

  | Run | Commit | Failed | Succeeded | Read p95 | Thresholds missed | Operations | Duplicates |
  |---|---|---|---|---|---|---|---|
  | 1 | `dbad75e` | 99.3% (95,481 of 96,120) | 639, about 13/s | 1,027 ms | error rate, `unexpected_status`, `dropped_iterations` (3,878), read p95 and p99, write p95 and p99: every threshold but `admission_refused` | 2,226 | 0 |
  | 2 | `f6da1a7` | 74.4% (74,057 of 99,490) | 25,433, about 509/s | 363 ms | error rate, `unexpected_status`, read p95 and p99, write p95 and p99 (passed: `dropped_iterations` at 501, `admission_refused`) | 7,104 | 0 |

  - Both missed the error-rate threshold (below 0.1%) by **740–990×**.
  - Run 1 collapsed: 1 s pool timeouts queued up to the 512 in-flight
    bound, and everything beyond was shed.
  - Run 2 followed three fixes: one round trip for each read and for the
    claim, shedding at 128 in flight (now tested), and error causes logged.
  - In both runs every operation drained, and the provider held exactly
    10,000 resources for 10,000 planned.

  A reads-only probe and a plumbing probe at 500 requests/s (read p95
  under 9 ms) were also taken. They live only in the git-ignored
  `target/perf/` and ran on uncommitted trees, so they are anecdotes, not
  evidence. The quiet-machine smoke and the 1,000,000-request run are
  **owed**.

What this says for the decision: reconciliation against a fake was made
correct within B0, and the review's adversarial cases (paused workers, late
creates) are now drilled. The cost lies elsewhere: real adapters, their
credentials, the extra threats and the components to operate. None of
those buys back the work the products repeat.

## 6. Versions verified on 2026-11-16

Each was read from the project's official site, GitHub releases or tags, or
official docs, by a delegated web check. Items marked *inferred* or
*unverified* are stated as such.

| Component | Version (date) | Note |
|---|---|---|
| PostgreSQL | 18.7 and 17.12 (2026-11-12) | supported majors 18, 17, 16, 15; 14 reached end of life with 14.25; 15 is supported until 2027-11-11; 19 is at RC 2 with GA planned for 2026-11-19 (postgresql.org) |
| Keycloak | 26.8.2 (2026-11-05) | the newest OpenID Foundation certification listing found is for Keycloak **18.0.0** (2022-05-19); D-019's "OpenID Certified" is therefore *unverified for 26.x* |
| MinIO community | last tag 2025-10-15 | README: "no longer maintained", source only; repository archived 2026-04-25. Excluded |
| Garage | v2.5.0 (2026-10-24) | AGPLv3; bucket quotas supported since v0.8. Read from the GitHub mirror; the primary forge was not readable |
| SeaweedFS | 4.54 (2026-11-16) | Apache-2.0; bucket quota listed |
| Podman | v6.2.0 (2026-11-10) | rootless Quadlet units under `~/.config/containers/systemd/` |
| OpenTelemetry Collector | v0.165.0 (2026-11-07) | 0.x; about one release every two weeks |
| Caddy | v2.11.9 (2026-11-05) | |
| Valkey | 9.1.3 (2026-10-26) | not used: no cache in the first release (D-007) |
| Grafana k6 | v2.4.0 (2026-11-12) | this repository pins v2.3.0 (checksum-verified); v2.4.0 is four days old |
| Supabase self-hosting | compose tag `self-hosted/v0.8.4` | about 11 services; "community-supported"; 4 GB RAM minimum; Studio supports one project and one organization; managed backups and PITR are not available self-hosted |
| Appwrite | 2.7.2 (12 November; year *inferred* 2026) | minimum 2 cores, 4 GB RAM, 2 GB swap; upgrades need `migrate` after a backup |
| Coolify | v4.7.5 (2026-11-10) | a running PaaS: the "platform" shape, already built |
| Kamal | v2.12.0 (2026-06-18) | a deploy tool: the "template" shape; no release in five months |

The control database in `deploy/compose.yaml` and CI uses
`postgres:18.7-trixie`, pinned by digest.

## 7. Recommendation

> **Recommendation: stay parked, and meet the need with a copyable starter
> template instead of a running control plane.** The three reference
> workloads are planned to run as one Rust binary each, behind Caddy on a
> 2 GB droplet with managed data and files. The slice they all repeat
> (accounts, workspaces, tokens, onboarding) can be met by copy-and-own code
> that each product runs on its own database. That is what D-011 allows and
> what D-012 asked for. A control plane would add all ten control-plane
> threats, five or more operated components and its own upgrade and restore
> drills, without removing any of that work. Even its leanest form, a
> reconciler CLI, provisions resources nobody needs provisioned today.

**Decision:** accepted with corrections on 2026-11-16 by the coordinator,
acting on the maintainer's delegation (D-036). In practice:

- `backend-platform` keeps its status "parked (future horizon)". B0's spine
  stays as the tested record of how reconciliation would be done. B1–B7 do
  not start.
- The next milestone is **B1-T, the starter template**
  (`docs/STARTER_TEMPLATE.md`). Its acceptance test has Durable Runner
  adopt the template on a fresh droplet faster than a timed hand baseline.
  Until that run exists, the template's benefit is a claim too.
- D-019's certification wording is corrected (§6), and MinIO is recorded as
  excluded (D-031).

**What would change the answer:**

1. **Provisioning on demand becomes a real workload.** For example, new
   apps or friends' projects needing their own database, bucket and realm
   arrive faster than they can be provisioned by hand. The point where
   hand provisioning costs more than a reconciler is **a guess, not a
   derivation**: perhaps three or more a month. It should be measured
   against the template's own provisioning steps once those are timed.
   No such demand exists today. If it appears, (c) the reconciler CLI comes
   before (b).
2. **The products leave managed services** for self-hosted PostgreSQL and
   object storage on shared hosts. Then per-project roles, quotas and
   isolation (T3, D-017) become everyday needs that a reconciler does
   better than runbooks.
3. **The template fails its own baseline.** If B1-T's acceptance run, or
   the D-028 operations script with the template, is not faster than doing
   it by hand, the case for automation reopens.
4. **People exist to validate it.** A second developer (P-3) and three
   outside teams (P-4) are the ship gate. Without them the platform cannot
   pass, however good the code is.
5. **Memory stops being the constraint.** A host of 8 GB or more per
   organization, with Keycloak's and the object store's idle footprints
   measured inside the budget.

**Still owed for B0's own gate (independent of the verdict):**

- a 100,000-request smoke on a quiet machine;
- the 1,000,000-request control-API run, only when the load average is
  below about 6.

Both busy-machine smokes are kept as recorded misses.
