# Roadmap

Planning-stage: every milestone is **Planned** and starts only when the
previous gate's evidence exists — one milestone per branch, merged when
its proof is stored. The guide's four implementation steps map as:
**1** integration case → B0 · **2** starter deployment → B1–B3 ·
**3** operations → B5 · **4** external validation → B7. The performance
and load plan is B6.

| Milestone | Builds | Proves (gate) | Status |
|---|---|---|---|
| **B0 — integration case + scaffold** | Workspace, toolchain, gates, this doc set; the evidence memo choosing **one** recurring developer problem demonstrated by the workspace's existing projects | The pattern is chosen from observed need, not preference; reproducible repo; `scripts/check.sh` runs in CI | Planned |
| **B1 — control plane core** | Control DB (desired/observed/operations/audit), project + environment lifecycle API, per-project principals, console shell, CLI skeleton | One API for console and CLI; admin actions audited; the desired/observed split visible in every status response | Planned |
| **B2 — reconciled provisioning** | The operation/step model, the lease-claimed worker, the starter's adapters (PostgreSQL database + role, S3 scoped path, OIDC client), observed-state reporting, fault fixtures | The partial-provisioning drill: a mid-operation failure keeps the real resource IDs and reconciles or compensates — no duplicate resources on retry. This drill gates every later adapter | Planned |
| **B3 — starter deployment** | Release manifests, the deploy operation, the container runtime adapter; the one application pattern end-to-end: create → provision → deploy → run | The starter actually runs; a recorded control-plane-down drill shows applications keep serving while management degrades honestly | Planned |
| **B4 — jobs + diagnostics** | The one background job (engine per the open question), request/job history via an OpenTelemetry pipeline, diagnosis views in console + CLI | The observe→diagnose path works on real traffic; quotas hold below the dashboard in the noisy-neighbor fixture; any cache is proven disposable | Planned |
| **B5 — operate** | Upgrade with version manifests + compatibility matrix, secret rotation, backup/restore/export — shipped as tested drills | A second developer runs upgrade, rotate, restore and export from docs alone; the interrupted-upgrade drill replays nothing blind — reconcile, then resume | Planned |
| **B6 — load + isolation evidence** | `perf/` scenarios: time to useful deployment, control-API latency, idle footprint, upgrade disruption; ten projects under mixed reads/jobs with one noisy neighbor; the control-plane-stopped availability matrix | Committed results (machine + commit hash) with misses kept, not rerun; budgets hold at the enforcement layer under the noisy neighbor | Planned |
| **B7 — external validation + ship gate** | Install/operate docs hardened by use; the supported component matrix, artifact provenance, the vulnerability process, migration paths | Three independent teams repeat the operations; the ship gate is met when integrations demonstrably save setup and recovery effort | Planned |

## Later directions (each needs its own contract + evidence)

Hosted multi-tenancy, metered billing, regions and stronger isolation
(independently reviewed threat model first), the jsrt executor, native
apps, additional adapters — and "safe AI workflows", which currently has
no contract at all. Expand only from observed need.
