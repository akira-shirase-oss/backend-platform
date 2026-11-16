# Backend Platform

> **Status — planning.** The contract is read and the docs below are the
> plan; no code exists yet. The guide marks this an *expansion horizon*:
> work proceeds one milestone at a time, each behind its own proof gate —
> it is not an approved simultaneous build or a commercial offering. See
> `docs/ROADMAP.md`.

A self-hosted backend platform in the Appwrite/Supabase direction — one
developer experience over deployment, identity, data, files, jobs and
diagnostics — assembled from proven components rather than rewritten
infrastructure. The point is to make small applications easier to
*operate*, not merely easier to scaffold: provisioning is a reconciled
process, running applications outlive the control plane, and upgrade,
restore and export are rehearsed drills, not runbook folklore.

- `docs/END_GOAL.md` — what done means, scope, invariants, the four
  required failure cases, the ship gate, open questions
- `docs/ARCHITECTURE.md` — sources of truth, components, paths,
  consistency story
- `docs/ROADMAP.md` — B0–B7 milestone order with per-milestone proof gates
- `docs/DECISIONS.md` — the guide-seeded trade-offs (D-001…)
- `AGENTS.md` — project rules on top of the workspace `AGENTS.md`
- `../../docs/Project_Guides/09_Backend_Platform.docx` — the contract; it wins
  on conflict

**What counts as done:** a second developer can stand up the stack,
create a project, deploy a versioned application onto provisioned
identity/database/files, observe its request/job history, then upgrade,
rotate secrets, restore and export — with the four contract failure cases
proven and the integrations shown, by independent teams, to save setup
and recovery effort.
