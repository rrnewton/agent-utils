---
title: 'wrkslots-dead-actor-journal: a provably dead actor''s journal never blocks unrelated mutations'
status: closed
priority: 1
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-05T22:24:35.086869239+00:00
updated_at: 2026-11-05T23:44:25.000000000+00:00
closed_at: 2026-11-05T23:44:25.000000000+00:00
---

# Description

A dead actor's interrupted-mutation journal blocks every wrkslots mutation on the host. On 2026-11-05 `wrkslots recover-absent-validate-rows` wrote `worktrees/ACTIVE.<host>.journal` (kind recover-absent-validate-rows, cursor 1, one item), then exited without finishing or cleaning up. Every unrelated mutation, validate creates included, then refused:

    REFUSED: another interrupted mutation is recorded in .../worktrees/ACTIVE.<host>.journal

A manual `wrkslots recover --coordinator-pid <pid>` fixed it ("archived with validation outcome unknown").

# Design

- When a mutation finds a journal whose actor generation is provably dead (the recorded pid and start time no longer match a live process, and no wrkslots process remains in its cgroup), it recovers the journal itself under the lock, the same as `wrkslots recover` does, and logs that. Then it proceeds.
- If proof is not possible (the actor may be alive), refuse as now, naming the actor and the recover command.
- At minimum, a journal for one slot or item must not block mutations of unrelated slots.

# Acceptance Criteria

- Test: a journal left by a killed actor (its pid gone) is recovered automatically by the next create, which then succeeds.
- Test: a journal whose actor is alive still refuses.
- Test: a dead actor's journal for item A doesn't block a mutation of unrelated slot B, if automatic recovery is deferred.

# Notes

Reported by the lead on 2026-11-05 at 5:40 PM ET (P1). The same family as #885 (orphaned create journal) and the seal deadlock. Assigned to the dedicated wrkslots lane.
