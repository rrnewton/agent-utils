---
title: 'wrkslots-validate-cleanup-no-progress: a validate-checkout cleanup worker makes no progress through wrkslots and churns the state lock'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- validate
- reclamation
- state-lock
created_at: 2026-11-28T15:20:00-05:00
updated_at: 2026-11-28T15:20:00-05:00
---

# Description

Reported on 2026-11-28 by a consuming team. Its validate-checkout cleanup worker drains finished validation checkouts and ends each drain with a wrkslots `retire-pending` batch. Its last run did three bounded sweeps and removed nothing: no checkouts, no Cargo homes, no runtime roots. The deferred paths made no progress in three consecutive sweeps. While it ran it churned the wrkslots state lock until lanes' heartbeats stalled behind it, so the consumer paused it with an operator marker. Tens of GB of terminal validation checkouts and build homes on the shared slot filesystem wait on the fix.

The worker's log (about 12,000 lines) gives the refusal counts:

- 2,334 "state lock is busy" refusals (state-lock contention);
- 1,184 census-bound refusals: 666 over the file-count bound and 518 over the MiB bound for retained validation handles;
- 193 read-only census refusals over the operation-wide output bound;
- 311 owner-generation refusals: 220 rows with no exact owner process generation, 91 with a recorded owner generation still live;
- smaller groups: an archived checkout path escaping its archived slot (60), privileged census diagnostics (59), an interrupted mutation that must be recovered first (47), a validation record with no evidenced terminal result (36).

Per-path reasons in the last result:

- 147 "git still registers this absent checkout; not retirable until the registration is pruned";
- 25 "no run record names it";
- 21 "in use by PID # mount table";
- 14 "batch limit".

The report also counts 39 new "reap incidents": a finished validation checkout held a tracked change or a HEAD other than its target when it was reaped. Those may belong to the consumer's own validate code rather than to wrkslots; the analysis should say which.

Related: #2392 (validate-slot unit-proof teardown: deterministic reclaim, and the hazard of a repository-wide `git worktree prune`), #2459 (wall-clock remove budget), #2065 (census cases that are load-sensitive on a hot host), #1247 / #1172 / #2468 (read-only scans and state-lock scope).

# Design

The consumer's requirement, not a design. Analysis first: map each refusal class to the wrkslots code path that raises it, with counts reproduced on a scratch registry of comparable shape.

1. Progress under contention: a cleanup batch takes the state lock for bounded work and releases it between items. It never starves heartbeats or other lifecycle mutations, and it makes forward progress when lock waits are short.
2. Absent-but-registered checkouts: retire a checkout whose directory is gone without a global `git worktree prune`. Prune exactly the one stale registration whose gitdir names that absent path, after proving the path absent, and record what was pruned.
3. Census bounds: validate checkouts carry large build trees, so the file-count, size and output bounds refuse them on every sweep. The bounds stay fail-closed, but a terminal validation checkout must be measurable and retirable within them (for example by scoping or streaming the census over known regenerable trees), with the basis recorded.
4. Owner generations: a row with no exact owner generation, or with a live recorded owner, is correctly refused. Refuse it once, by name, in the batch report, and do not re-attempt it every sweep as churn.
5. Every refusal stays fail-closed and named. No retirement is forced past a live owner, a tracked change or a mount.

# Acceptance Criteria

- Each refusal class above is traced to its code path with file:line, and reproduced or explained on a scratch registry; the reap-incident question is answered (wrkslots or the consumer's code).
- A cleanup batch never holds the state lock across more than one item's bounded work. A test shows a heartbeat and an exclusive mutation proceed while a batch runs (condition-based, no wall-clock assertion).
- An absent checkout with a stale registration is retired by pruning only its own registration. A test shows other registrations, including ones whose directories are momentarily absent, are untouched.
- A terminal validation checkout over today's census bounds is measured and retired. A test shows the bounds still refuse an unclassifiable or live tree.
- A refused row is reported once per batch with its reason and is not retried within the batch. The batch report counts progress and refusals separately.
- Existing validate-reclaim, retire-pending, census and lifecycle tests pass.

# Notes

Report evidence only at filing time, restated from the consumer's defect package; no wrkslots analysis yet. Number pre-assigned by the coordinator (2026-11-28); the fix is staffed separately.
