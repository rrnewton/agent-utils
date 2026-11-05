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
- The pre-deletion salvage and inventory work (`git ls-tree -r`, `git rev-parse --git-common-dir`, remote checks) runs OUTSIDE the lock. Under the lock, only a cheap re-validation runs, for example HEAD, ref and mtime fingerprints; if they changed, redo the work outside the lock. Alternatively the inventory becomes cheaper than a full `ls-tree`. Either way, a plain `wrkslots remove` (without --no-lock-budget) of a cleaned multi-GB slot under load no longer trips the 60 s pre-deletion budget.
- A remote check that needs a GitHub credential while the credential-pacing bucket is full WAITS, bounded, for a credential slot instead of refusing. If the wait times out, it refuses with a clear "retry later: credential pacing" message. A test covers both a wait that succeeds and one that times out.

# Notes

Reported by the lead on 2026-11-05 at 10:46 AM ET, from a consumer team (P1; it starves validation admissions). Assigned to mu-validate after #879.

More data from the same consumer team, via the lead on 2026-11-05 at 11:39 AM ET:
- A plain `wrkslots remove` refused a cleaned 6.5 GB slot twice on the 60 s pre-deletion budget under load. The first time the budget ran out on `git rev-parse --git-common-dir` in the shared repo; the second time on `git ls-tree -r --name-only` in the slot's checkout.
- Another slot refused because the pre-salvage remote check needed a GitHub credential while the git_credential pacing bucket was full ("credential helper ... told us to quit").

The last two acceptance items were added for these.
