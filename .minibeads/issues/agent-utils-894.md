---
title: 'wrkslots-remove-lock-scope: remove holds the run-population lock only for state transitions, not deletion I/O'
status: open
priority: 1
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T15:46:39.396859633+00:00
updated_at: 2026-11-05T15:46:39.396859633+00:00
---

# Description

P1. `wrkslots remove --coordinator-authorized --expected-generation 1 --no-lock-budget <slot>` held the project state root's run-population lock for about 9 minutes, for one cleaned 3.2 GB slot (2026-11-05, 10:31-10:40 ET, a consumer host).
- Every validation admission (validate-lock run-population) waited behind it.
- The next removal took the lock again at once, starving admissions further.

# Design

- `remove` holds the lock only for state transitions: fence the slot directory, write the journal, update the registry. It does not hold it across deletion I/O or the `lsof +D` walks.
- The safety checks must hold at the moment of fencing. Once the directory is fenced (renamed out of every managed path), deleting it needs no lock. Re-take the lock only to record completion.
- This applies under `--no-lock-budget` too. That flag lifts the time budget; it does not license holding the lock through I/O.
- Fairness: back-to-back removals yield to waiting admissions. For example, a removal releases the lock and does not re-acquire it ahead of a queued waiter (a ticket or turn order, or a short yield that lets waiters in).
- Both editions.

# Acceptance Criteria

- Test: while a large fenced slot directory is being deleted, another process acquires and releases the run-population lock promptly, within seconds, not after the deletion.
- Test: two queued removals plus a waiting admission. The admission gets the lock between the removals.
- A removal of a multi-GB slot holds the lock for seconds, as measured in the test or a timed run.

# Notes

Reported by the lead on 2026-11-05 at 10:46 AM ET, from a consumer team (P1; it starves validation admissions). Assigned to mu-validate after #879.
