# B0 memo — a starter template or a running control plane?

Written 2026-11-16 for the maintainer's decision (README P-2, D-012). It
answers the identity question first (D-011), describes what one small
organization actually needs to run its small apps, measured against the
portfolio's own products, and then compares two ways to meet that need. It
ends with a recommendation, marked as such, and the conditions that would
change it.

Facts are dated. Component versions were read from each project's official
source on 2026-11-16 (§6). Measurements come from this repository's B0 spine
and carry the commit and machine load (§5). Anything not measured says so.

## 1. The identity question, answered first

**The platform cannot remove the work the products repeat most.** Each
product's `docs/SAAS_PLAN.md` (written 2026-11-13) lists the same gap rows
between "my tool" and "SaaS": accounts, invite-gated sign-up, email
verification, workspaces, members and roles, per-workspace tokens or API
keys, plan limits, onboarding and account lifecycle. That slice is the
recurring developer problem D-012 names. D-011 forbids the platform from
supplying it, because a shared login would make all three products depend
on one service: the guide's dependency trap. The products are independent
by the maintainer's decision of 2026-11-13. The platform could provision
identity *per project* (one OIDC realm each), but:

- Colophon keeps Keycloak off its droplet for memory reasons and runs
  password accounts only for the beta (Colophon `SAAS_PLAN.md`, "Keycloak");
- Service Monitor and Durable Runner plan their own accounts on Neon.

So the identity half of the platform has no consumer among the reference
workloads. What remains to compare is the *operations* slice: deploy, data,
files, jobs, diagnostics, backups, upgrades and restore.

## 2. What the reference workloads actually need

The three products stay independent (D-011). They are measured here as
workloads, not proposed as tenants.

| | Colophon | Service Monitor | Durable Runner |
|---|---|---|---|
| Process | one Rust binary (API + Studio + worker loop) | one Rust binary + static status page | one Rust binary (API + dashboard + engine) |
| Host | one 2 GB droplet, Caddy → systemd | its own 2 GB droplet (observes the others from outside) | one 2 GB droplet, Caddy → systemd |
| Durable data | Neon Postgres (LISTEN/NOTIFY, advisory locks, `SKIP LOCKED`) | SQLite on the droplet (evidence) + Neon (accounts) | append-only journal on a volume (the authority) + Neon (accounts) |
| Files | S3 API: Spaces or R2 planned; Garage only in the dev stack | none | none |
| Cache | Valkey (capped) for quotas | none | none |
| Jobs | in-process (scheduled publishing stays inside, D-060) | the probe loop, in-process | it *is* the job runner |
| Backups today | `pg_basebackup`/`pg_verifybackup` (cannot run on Neon; PITR planned) | `VACUUM INTO` + `backup-drill.sh` | `/admin/backup` + restore drill |

Sources: each product's `docs/SAAS_PLAN.md`; `products/colophon/deploy/compose.yaml`;
`products/service-monitor/deploy/`.

What they repeat, measured as artifacts that already exist per product:

- Colophon: `backup.sh` (719 lines), `restore.sh` (733), `upgrade-rehearsal.sh` (178),
  `install.sh` (154), `failover.sh` (110), `backup-crypt.sh` (74), plus runbooks for
  backup, restore, upgrade, observability and install (594 lines).
- Service Monitor: `backup-drill.sh` (85), `deploy.sh` (80), `RUNBOOK.md` (162),
  a hardened `monitor-server.service` (60).
- Durable Runner: `docs/OPERATING.md` and its backup and restore endpoints.

None of them needs a database, a bucket or a realm *provisioned on demand*.
Each needs the same handful of operating chores done well, once per product:
TLS and a reverse proxy, a hardened service unit, an environment and secrets
file, backups with a restore drill, an upgrade procedure with a rehearsal,
health and readiness endpoints, and logs. The managed services (Neon, Spaces
or R2) already provide the durable data and files.

## 3. The two options

**(a) A copyable starter template.** A directory that each product copies
and then owns, the same copy model the products already use for
`@colophon/ui`. It holds a Caddyfile; hardened systemd units; an environment
template with secret-file conventions (mode 0600, never in logs); backup,
restore and upgrade scripts, each with its drill; a health and readiness
contract; and the CI and load harness this repository now has (pinned
toolchain, `check.sh`, `cargo deny`, SHA-pinned actions, a k6 runner that
records machine and commit). Nothing runs that the product does not already
run.

**(b) A running control plane.** What `ARCHITECTURE.md` describes: a control
database, an API, a reconciler worker holding provider admin credentials,
adapters for PostgreSQL, the object store, the OIDC provider and the
container runtime, a console, a CLI, an OTLP collector and a diagnostics
database. It is assembled from proven components, but someone has to run it.

## 4. Comparison

### Reliability

| | Starter template | Control plane |
|---|---|---|
| New runtime components | none | control DB, API, worker, console; plus Keycloak, an object store, Podman and an OTel Collector if self-hosted |
| In the app's request path | nothing new | nothing by design (invariant 3); proving it is the `control-plane-down` drill, owed at B3 |
| Provisioning failure modes | none: resources are created once, by hand or by the provider's console | partial provisioning, lost replies, crashed workers. B0 proves convergence against the **fake** provider only; the real adapters are B2 |
| Overload behaviour | each product's own | the control API sheds with 503/429 instead of hanging (seen in the B0 smoke, §5); still one more thing to size |

### Upgrade and restore drills

- **Template:** one upgrade drill and one restore drill per product, for that
  product's own data. Colophon and Service Monitor already have both; the
  template standardizes them.
- **Control plane:** every product's drills stay, **and** the platform's own
  are added. Failure case 4 (D-027) has four interruption points, and a
  restore must reconcile observed state before any job resumes. That is more
  drills, not fewer, for as long as the platform holds no app data itself
  (and it must not).

### Security surface (threat model T1–T10, `ARCHITECTURE.md`)

| # | Threat | With the template | With the control plane |
|---|---|---|---|
| T1 | compromised console/API reaches every project | does not exist: there is no central API | exists; mitigated by the API/worker split (CI-checked since B0) |
| T2 | compromised worker holds the keys | does not exist: no process holds admin credentials over other products | exists: the crown jewel |
| T3 | cross-project access | separated by default: separate droplets, Neon projects, buckets | must be enforced by roles, keys, realms and networks on a shared host |
| T4 | container escape | no shared container host | rootless Podman on a shared host |
| T5 | admin abuse, repudiation | provider audit logs (DigitalOcean, Neon) | an append-only audit to build and protect |
| T6 | secret disclosure | per-droplet secret files, blast radius of one product | envelope encryption and a KEK whose loss loses every secret |
| T7 | SSRF via user URLs | not in a control path (Service Monitor already guards its own egress) | an egress guard to build |
| T8 | supply chain | Caddy + the product | + PostgreSQL server, Keycloak (JVM), object store, Podman, OTel Collector |
| T9 | backup theft, malicious restore | per product | central backups: one prize, one restore path for everything |
| T10 | control-plane DoS | does not exist | exists |

The template leaves 3 of the 10 threats (T6, T8, T9), each with a
one-product blast radius. The control plane carries all ten.

### Operating effort

Neither option's effort has been timed. The honest measure is the D-028
baseline (the operations script done by hand), and nobody has run it yet.
What can be counted:

- **Template:** the products already carry about 2,950 lines of operations
  scripts, units and runbooks (§2: Colophon about 2,560, Service Monitor 387). A template would replace the duplicated parts
  with one maintained copy per product. It adds no service to patch.
- **Control plane:** B0's spine alone is about 5,600 lines of Rust (unit
  tests included) plus about 1,460 lines of integration tests and drills,
  and it provisions nothing real yet. Running it adds the
  upkeep of every adapter. The guide says to budget an adapter's maintenance
  before adding it: an owner, a pinned version and an advisory watch. The
  OTel Collector is still 0.x, with a release about every two weeks;
  Keycloak is a JVM service with its own advisories to track.
- **Memory:** each product has a 2 GB droplet. The two products in this
  space publish self-hosting minimums above that before any app runs:
  - Supabase: 4 GB RAM, 2 cores, 40 GB SSD.
  - Appwrite: 4 GB RAM, 2 cores, 2 GB swap.

  Colophon already left Keycloak off its droplet for memory. The control
  plane's idle footprint is unmeasured (a B2 gate).

### Proven components each would use (verified 2026-11-16, §6)

- **Template:**
  - Caddy 2.11.9 and systemd.
  - The products' managed services (Neon; Spaces or R2).
  - For self-hosters: PostgreSQL 18.7 and Garage v2.5.0.
  - k6 for load.
  - PostgreSQL's own `pg_dump`/`pg_basebackup` for backups.
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

- **Drill `partial-provision`, in process:** 10,000 randomized fault
  schedules (`crates/platform-worker/tests/partial_provision.rs`). They
  injected:
  - 41,855 provider faults: failures before any effect, lost create replies,
    and calls that hang past the deadline after taking effect;
  - 19,510 simulated worker crashes at every fault point;
  - 9,150 partial rounds, each retried;
  - 3,018 pre-existing resources, all adopted.

  The schedules planned 50,027 resources. Every operation converged with
  **0 duplicates and 0 orphans**. 11,123 resources were adopted after a lost
  reply or a crash instead of being created twice. The fake deliberately
  allows duplicate names, so only the reconciler's observe-before-create rule
  prevents them. A blind-retry reconciler is caught by the same oracle.
- **Drill `partial-provision`, process level**
  (`crates/platform-worker/tests/kill_drill.rs`):
  - The real worker binary on PostgreSQL was killed with SIGKILL (exit
    signal 9 asserted) at three points of each of the five steps of the
    starter plan. The points are: after the attempt is committed, after the
    lookup, and after the create took effect but before it was recorded.
    A sixteenth kill came after the finish.
  - In 20 more cases the worker was not killed; instead every provider fault
    hit every resource kind.

  All 36 cases converged to exactly 5 provider resources, with every
  recorded id matching the provider's. After a kill that followed a create,
  the next worker adopted the resource: 5 creates for 5 resources.
- **Control-API load smoke**
  (`perf/results/`, 100,000-request scenario, 80% reads and 20% writes, at
  2,000 requests/s offered against the fake provider with 0.1% faults). Both
  runs were taken on an Apple M2 Pro laptop (12 cores, 32 GB) shared with
  other builds. Its load average was **25–65**, well above the quiet-machine
  bar of about 6, so both are busy-machine observations, not capacity. Both
  runs **missed their latency thresholds and are kept**:

  | Run | Commit | Failed | Succeeded | Read p95 | Operations | Duplicates |
  |---|---|---|---|---|---|---|
  | 1 | `dbad75e` | 97% | ~53 requests/s | 1,027 ms | 2,226 | 0 |
  | 2 | `f6da1a7` | 73% | ~435 requests/s | 363 ms | 7,104 | 0 |

  - Run 1 collapsed: 1 s pool timeouts queued up to the 512 in-flight bound,
    and everything beyond was shed.
  - Run 2 followed three fixes: one round trip for each read and for the
    claim, shedding at 128 in flight, and error causes now logged.
  - In both runs every operation drained and the provider held exactly
    10,000 resources for 10,000 planned.

  A reads-only probe failed the same way at that load, so the limit was the
  saturated host, not the write path. A plumbing probe at 500 requests/s
  passed with a read p95 under 9 ms. The quiet-machine smoke and the
  1,000,000-request run are **owed**.

What this says for the decision: reconciliation against a fake was made
correct, and proven, within B0. The cost lies elsewhere: real adapters, their credentials,
the extra threats and the components to operate. None of those buys back
the work the products repeat.

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
> workloads each run one Rust binary behind Caddy on a 2 GB droplet with
> managed data and files, and the slice they all repeat (accounts,
> workspaces, tokens, onboarding) is exactly what D-011 keeps a platform from
> supplying. A control plane would therefore add all ten control-plane
> threats, five or more operated components and its own upgrade and restore
> drills while removing none of that work. A template removes the
> operational duplication with nothing new to run.

What this means in practice, for the maintainer to accept or reject (P-2):

- `backend-platform` keeps its status "parked (future horizon)". B0's spine
  stays as the tested record of how reconciliation would be done. Nothing
  past B0 starts.
- The template is a separate piece of work, outside this repository and not
  started here. Its first version is assembled from what Colophon and
  Service Monitor already have.
- D-019's certification wording is corrected (§6), and MinIO is recorded as
  excluded (D-031).

**What would change the answer:**

1. **Provisioning on demand becomes a real workload.** For example, new
   apps or friends' projects needing their own database, bucket and realm
   arrive faster than they can be provisioned by hand. At three or more a
   month, hand provisioning starts to cost more than running a reconciler.
   No such demand exists today.
2. **The products leave managed services** for self-hosted PostgreSQL and
   object storage on shared hosts. Then per-project roles, quotas and
   isolation (T3, D-017) become everyday needs that a reconciler does
   better than runbooks.
3. **The template fails its own baseline.** If the D-028 operations script
   with the template is not faster than doing it by hand, the case for
   automation reopens.
4. **People exist to validate it.** A second developer (P-3) and three
   outside teams (P-4) are the ship gate. Without them the platform cannot
   pass, however good the code is.
5. **Memory stops being the constraint.** A host of 8 GB or more per
   organization, with Keycloak's and the object store's idle footprints
   measured inside the budget.

**Still owed for B0's own gate (independent of the verdict):**

- the maintainer's acceptance of this memo (P-2);
- a 100,000-request smoke on a quiet machine;
- the 1,000,000-request control-API run, only when the load average is
  below about 6.

Both busy-machine smokes are kept as recorded misses.
