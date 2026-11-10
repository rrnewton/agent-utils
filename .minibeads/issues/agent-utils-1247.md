---
title: 'wrkslots-status-lock: a read-only status holds the global state lock across a long scan'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- state-lock
created_at: 2026-11-10T21:27:58+00:00
updated_at: 2026-11-10T21:27:58+00:00
---

# Description

[opus 5] P1 defect report from a consumer lead (2026-11-10, about 4:24 PM ET), live on the shared host now. `wrkslots --wait-lock 600 status`, run by a reviewer lane at about 3:43 PM ET, held the `ACTIVE.global.lock` file (fd 3) for about 40 minutes in state R at full CPU. Every wrkslots mutation on the host was refused (`state lock is busy for .../worktrees/ACTIVE`), including slot creates. The hourly audit service failed (exit 2), the observer service timed out, and a health tick timed out at 651 s. A read-only `status` must not hold the global state lock across a long scan, or the scan must be bounded. The report notes the slot table reported `retained_storage_inconsistencies=9`, which may be what makes the scan slow; that is a hypothesis to test, not an established cause.

Related: #786 agentctl-start-slot-bind-busy-lock (a `status` read refused on a busy lock), #1172 audit-lock-hold (phased readers: snapshot under a short lock, measure outside it), #1194 wrkslots-lock-scope (salvage and census before the configuration lock), #1108 remove-lock-hold-starves-validate, #1213 wrkslots-state-lock-deletion (slow removal phases outside the ACTIVE locks). None of those covers a read-only scan spinning under the lock or a wall bound on read-only scans.

# Design

1. Root cause first: why `status` takes 40+ minutes at full CPU. Suspects: a scan that is quadratic or worse in inconsistency findings, slots, Git worktree registrations or processes; a census retry loop; repeated Git subprocesses per finding. Reproduce on a local registry (which also reports dozens of retained-storage inconsistencies and a slow `status`) and record where the time goes before changing anything.
2. Lock scope: read-only commands (`status`, `doctor`, `audit`, `unpushed`) never hold the global state lock across a scan. They snapshot the state under a short lock (well under 1 s), then scan unlocked; anything they report comes from that snapshot.
3. Wall bound: a bound on read-only scans (default a few minutes, configurable), after which the command reports partial results with a clear marker and exits non-zero instead of running on.
4. Speed: fix the slowness itself where the root cause allows (for example cache per-repository Git worktree lists, skip repeated census work).

# Acceptance Criteria

- Root cause recorded in Notes with before/after measurements (operation or subprocess counts, never wall time alone).
- A lock-contention test: a `status` scan in progress does not block a mutation beyond the snapshot window.
- A large-fixture test of the algorithmic fix that counts operations or subprocess calls and never asserts on wall time.
- `status`, `doctor`, `audit` and `unpushed` each snapshot under a short lock and scan unlocked; a scan past its wall bound reports partial results with a clear marker and a non-zero exit.
- Existing lifecycle, audit, census and lock-scope tests pass.

# Notes

Report evidence only at filing time; no measurement has been made yet. The spinning process had exited before this lane could read its stack, so the root cause must be reproduced on a local registry.
