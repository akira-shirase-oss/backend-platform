# Decisions

Deliberate choices, newest last. D-NNN numbering follows the workspace
convention; D-001 through D-010 seed the contract's prescribed
directions — each is a real trade-off, not a default. Open questions the
guide leaves unresolved are listed in `END_GOAL.md`, not decided here.

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
  the blast radius small.
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
  about it.
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
