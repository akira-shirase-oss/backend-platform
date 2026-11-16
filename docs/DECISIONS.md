# Decisions

Deliberate choices, newest last. D-NNN numbering follows the workspace
convention; D-001 through D-010 seed the contract's prescribed
directions — each is a real trade-off, not a default. D-011 through D-028
were decided before any code, to settle the guide's open questions with
conservative defaults and to fix gaps a plan review found. What still
needs the maintainer or an outside party is under *Open questions (still
open)* and in the README's *Blocked on* table.

- **D-001 — Assemble proven components; rewrite nothing operational.**
  Identity, data, files and jobs come from mature dependencies
  (PostgreSQL, a reviewed OIDC integration, S3-compatible storage,
  standard containers). The platform's own code is the control plane —
  lifecycle, reconciliation, evidence. Trades stack uniformity for
  operability: the goal is to operate small applications, not to
  demonstrate a stack.
- **D-002 — Desired and observed state are separate truths.** The control
  database stores desired configuration; actual resources report observed
  state, and displayed status is derived from the join. `partial`,
  `uncertain` and `unknown` are first-class outcomes. Costs a harder read
  model and more bookkeeping; buys the property that the platform never
  confidently displays a state it does not know.
- **D-003 — Provisioning is a reconciled process, not a transaction.**
  Cross-provider atomicity does not exist, so operations carry IDs and
  recorded steps claimed under leases; retries resume the operation and
  re-check observed state. Compensate or reconcile — never a blind retry
  that creates a second database. Accepts eventual consistency as the
  honest model.
- **D-004 — Products run independently before integration.** Shared
  contracts are limited to identity, IDs, events and health; no shared
  mutable database across unrelated products; a failed control UI must
  not stop already running applications. Accepts some duplication to keep
  the blast radius small. *Refined by D-011: a shared identity contract
  means a standard (OIDC tokens, ID formats), never a shared login
  service.*
- **D-005 — Single organization first; multi-tenant is a separate
  evidence problem.** Per-project isolation is real (principals, database
  roles, scoped storage paths, isolated secrets, network boundaries), but
  hosted multi-tenant service, metered billing, regions and
  hostile-tenant workloads require independently reviewed threat modeling
  and capacity evidence — an extra deployment flag is not evidence.
- **D-006 — Standard containers are the workload; jsrt stays optional.**
  Standard application containers carry the first release's isolation
  story; the custom JavaScript runtime may become an executor later.
  Trades density for proven operability and familiar primitives.
- **D-007 — Cache is an accelerator, never an authority.** Valkey or
  another Redis-protocol cache only where evidence justifies it; never
  the sole durable job store; nothing reads it as truth. The first
  release may be slower than a cache-heavy design, and will be honest
  about it. The first release has no cache at all.
- **D-008 — Quotas enforce below the dashboard.** Per-project budgets
  bite at admission and at the workload boundary; dashboard counters are
  displays, not enforcement. A noisy neighbor is throttled by the
  mechanism, not by a metric someone is watching.
- **D-009 — Expand only from observed need.** One application pattern
  until three independent teams have repeated the operations and their
  observed friction says what to build next. The ship gate is saved
  effort, not surface area.
- **D-010 — Layout and conventions mirror the sibling projects.**
  `crates/` (domain, db, server, worker, adapters, cli), `apps/`,
  `packages/`, `deploy/`, `fixtures/`, `perf/`, `docs/modules/`,
  `scripts/`; the workspace toolchain pin (Rust 1.99.1) and lint set;
  `platform-domain` holds no I/O, enforced like the other projects'
  domain crates. The `platform-` crate prefix is provisional until the
  project gets a real name.

## Decisions made before code (2026-11-16)

Each states the choice, the reason and what it costs. Numbers are proposed
starting values; changing one needs a measurement and a new entry.

- **D-011 — The platform is never the shared login for the existing
  products.** The portfolio's products (Colophon, Service Monitor, Durable
  Runner) are independent: each keeps its own accounts, login, workspaces,
  tokens and databases, and they integrate only through public APIs. They
  are not tenants of this platform and depend on nothing it provides.
  Identity the platform provisions is per project — one OIDC realm per
  project, no single sign-on across projects. Reason: those products'
  independence was decided on 2026-11-13, and a platform that supplied
  their identity would make all of them depend on it — the guide's own
  "dependency trap". Cost: the account and workspace work each product
  repeats is not removed by this project.
- **D-012 — B0's memo compares a starter template with a platform, and may
  recommend staying parked.** The recurring developer problem visible in
  the existing projects is the accounts, workspaces, tokens and onboarding
  slice each product builds for itself. The memo first answers the identity
  question (D-011), then compares (a) a starter template each project copies
  and then owns — the same copy model the products already use for their UI
  — with (b) a running control plane. If (a) saves the effort, the honest
  outcome is the template, and this project stays parked. Reason: the guide
  says expand only from observed need. Cost: the likely result is that no
  platform is built soon.
- **D-013 — The control plane has its own threat model.** Recorded in
  `ARCHITECTURE.md` (T1–T10) with a test per mitigation. Its core rule:
  provider admin credentials exist only in the reconciler worker, which
  opens no listening socket; the API process holds none and does not link
  the adapters, and the worker re-checks every operation's authority before
  a provider call. Reason: the adapters' credentials are root-equivalent
  over every project, and a single-organization tool still has a console
  on the network. Cost: two processes and a policy check run twice.
- **D-014 — Least-privilege provider credentials.** PostgreSQL: a
  `platform_provisioner` role with `CREATEDB` and `CREATEROLE`, never
  `SUPERUSER` (PostgreSQL 16 or later, where such a role administers only
  the roles it created); each project role is `LOGIN` only, owns one
  database, has `CONNECT` revoked from `PUBLIC` on every other database,
  and gets a `CONNECTION LIMIT` and a `statement_timeout`. Object store: the
  provisioner key manages buckets, quotas and policies; each project key is
  limited to its bucket. OIDC: the admin client is used only by the worker.
  Container runtime: rootless, under a dedicated user. Cost: some
  operations that a superuser would do in one step need two.
- **D-015 — Secrets are envelope-encrypted and write-only.** A per-project
  data key encrypts that project's secrets; a key-encryption key in a file
  outside the control database (mode 0400, owned by the worker user)
  encrypts the data keys. The API accepts secrets but never returns them;
  they are absent from operation steps and logs, and a log-capture test over
  every drill fails on known secret shapes. Workloads receive secrets as
  files at start. Rotation is a B5 drill. Cost: losing the key file loses
  every secret, so its backup is part of the restore drill.
- **D-016 — Provider resource names are deterministic.** Each provisioned
  resource is named from the project ID and the resource's logical name
  (for example `p_<project>_<name>` for a database), and tagged with the
  operation ID where the provider supports it (`COMMENT ON DATABASE`,
  bucket tags, realm attributes). Every create step looks the name up first
  and adopts what exists. Reason: this is the mechanism behind "never
  create duplicate databases on a blind retry" — a lost reply or a restored
  control DB cannot lead to a second resource. Cost: renaming a resource is
  a migration, not an edit.
- **D-017 — Quota enforcement per resource.** CPU and memory: cgroup v2
  limits per container (rootless, with delegated controllers). PostgreSQL:
  per-role `CONNECTION LIMIT` and `statement_timeout`; database size has no
  native quota, so the reconciler measures it and sets an over-quota
  database to `default_transaction_read_only` — recorded as delayed
  enforcement. Storage: bucket quotas enforced by the object store (a
  selection criterion for it). Jobs: a per-project concurrency limit for
  the app's job runner and a per-project cap on in-flight platform
  operations, refused at admission. The noisy-neighbor drill reads these
  enforcement counters, never the dashboard. Cost: database-size limits
  are not instant.
- **D-018 — What the 1,000,000-unit runs count.** 1,000,000 control-API
  requests (80% reads, 20% desired-state writes) against the platform,
  with the worker reconciling against the fake providers; and 1,000,000
  app-facing requests across ten starter projects during the
  noisy-neighbor drill. Real providers get a separate count — 1,000
  provision and deprovision cycles — reported on its own and never
  extrapolated. A million real `CREATE DATABASE` runs is not a meaningful
  laptop workload, and no hosted third-party service is ever load-tested.
  This replaces "provisioning runs" as a million-scale unit.
- **D-019 — First OIDC provider: Keycloak, one realm per project.** It is
  mature, self-hosted, OpenID Certified and has an admin API that the
  worker can drive. The review bar is our own negative tests (exact
  redirect-URI match, PKCE required for public clients, issuer and audience
  checks, token lifetimes) plus a pinned supported release with its
  advisories tracked. Its idle footprint is measured at B2; if it breaks the
  footprint budget, a new entry revisits the choice. Cost: a JVM service is
  heavy on a small host.
- **D-020 — Containers: rootless Podman on one host, no orchestrator.** One
  container per app process, restarted by systemd user units; images pinned
  by digest. The control plane is not in any app's request path, so
  stopping it leaves apps serving. Reason: the first release is a
  single-organization starter; a scheduler adds a second control plane to
  operate. Cost: no multi-host scheduling or failover.
- **D-021 — The starter's background job uses the app's own database.** A
  Postgres-backed job table in the project's provisioned database, claimed
  with `FOR UPDATE SKIP LOCKED` under leases with attempt counts — a mature
  pattern with no extra service. The platform's own operations use the
  control DB's operation table. Other independent products (such as a
  durable workflow runner) are not dependencies; one could become an
  optional adapter later through its public API. The cache is never the job
  store. Cost: no cross-project job dashboard beyond diagnostics.
- **D-022 — Diagnostics: OTLP into a separate, bounded database.** Apps
  emit OTLP to an OpenTelemetry Collector on the host, which writes request
  and job history to a diagnostics PostgreSQL database — not the control DB
  — with 7-day retention and a size cap, behind a port. When it is full or
  down, the collector drops with counters and apps are unaffected. A
  dedicated trace store waits for a measured need. Cost: weaker trace
  queries than a purpose-built store.
- **D-023 — SDKs come from a checked-in OpenAPI 3.1 document.** The
  document is generated from the server's route and type definitions and
  committed; CI fails when the generated document drifts from the
  committed one. The TypeScript SDK comes first; the Rust CLI shares the
  server's types. Cost: the API is shaped by what OpenAPI expresses well.
- **D-024 — No AI features until a contract exists.** "Safe AI workflows"
  is named by the guide as a direction with no contract; it is excluded
  from the first release. Any later contract keeps the rule that agents
  propose and the platform enforces.
- **D-025 — A tenant is a project inside one organization.** "Ten projects"
  in the load plan means ten projects of one organization on one host.
  Organizations as tenants is the hosted multi-tenant direction, which is
  out of scope.
- **D-026 — Milestone 0 is the memo plus the spine, and the roadmap cites
  evidence.** B0 ships CI (fmt, clippy, tests, architecture checks,
  `cargo deny`), the fake provider, the first load scenario and the
  partial-provisioning drill along with the memo, so the first commit
  already proves the hardest property. A status cell says *Done* only with
  named tests and an evidence path.
- **D-027 — Named interruption points for upgrade and restore.** The drill
  fixtures stop the process at (a) after the control-DB migration, before
  the binary swap; (b) during the binary swap; (c) after the control-DB
  restore, before reconciliation; (d) after reconciliation, before jobs
  resume. Each must recover by re-running the same command from the
  recorded step, and the operation log must show reconciliation finishing
  before any job resumes.
- **D-028 — The ship gate in testable form.** Independent means developers
  who did not write the platform, from three different teams, using only
  the published docs; any founder intervention fails that operation. The
  operations script is fixed (install, create, provision, deploy, observe,
  diagnose a seeded failure, upgrade, rotate, restore, export), and its
  hand-assembled baseline is timed before any team starts. Pass: every
  team completes every operation, the median total time beats the
  baseline, and each recovery operation is individually faster. B5's
  second developer is the rehearsal, not a substitute.

## Open questions (still open)

These need the maintainer or an outside party; each is in the README's
*Blocked on* table.

- **Unparking (P-1)** — whether and when the project starts; the guide calls
  it a future horizon.
- **The application pattern and the template-versus-platform answer
  (P-2)** — the B0 memo proposes; the maintainer accepts or keeps it parked
  (D-012).
- **People (P-3, P-4)** — a second developer for B5 and three independent
  teams for B7.
- **Hostile multi-tenancy (P-5)** — an independent threat-model review
  before any shared-hosting direction.
