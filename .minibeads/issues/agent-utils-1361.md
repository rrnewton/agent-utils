---
title: 'create-provision-unlocked: create runs fetch, worktree adds and hooks inside its locked commit'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- lock-scope
created_at: 2026-11-12T11:42:14.000000000+00:00
updated_at: 2026-11-12T11:42:14.000000000+00:00
---

# Description

[watermelon-full-preview] After #1276 adopt-lock-narrow and #1287 retire-lock-narrow, a plain `wrkslots create` validates and scans the fleet with no lock held and takes one exclusive round -- but inside that round it still runs `_create_plan` (including `fetch_remote` per repository), `git worktree add` for every checkout, and `_run_post_provision_hooks`. Counted on a fixture (2 existing rows, one trivial hook): each of plan+fetch, target-clear, worktree-add and hooks runs exactly once while the flock on the project root is held. On a measured host whose post-provision hook runs a multi-GB `git submodule update --init`, two back-to-back creates held that flock for 2 min 9 s and 2 min 35 s (2026-11-11), blocking the same consumers as the retire-pending hold. #1123 create-lock-hold-classification-cost records the same shape at 76,008 events and proposes the reservation/provisioning/record design; its classification part is covered by #1276 and #1287, and this issue is the remaining provisioning part.

# Design

Move provisioning out of create's locked commit, following the #1123 design (which still needs the journal-format decision recorded there):

- Reservation (locked, short): re-verify config, global state, interrupted creates unrelated, slot/agent/branch free, fetched start points, disk floor and target clear; write the CREATE journal naming the creating process's own identity and its first progress event.
- Provisioning (unlocked): fetch per repository first (before the reservation, re-verified under it), then image/subvolume work, `git worktree add`s, post-provision hooks and git identity. Each journal progress rewrite re-takes the locks only to append its progress event (append-sized holds), asserting the journal's operation id and creator identity are still this process.
- Record (locked, short): re-verify journal ownership and completeness, checkout identities unchanged, and create processes live; publish the ACTIVE row and completion event; the only-this-slot-changed check tolerates disjoint rows other creates added meanwhile.
- Busy semantics: a CREATE journal whose creator is live is provisioning, not interrupted; a related waiter (same slot, agent, branch, or path) refuses as busy naming the creator instead of classifying it interrupted; a dead creator recovers exactly as today. Failure during unlocked provisioning leaves a removable record (the journal plus recovery), never a half-published slot.
- Tests count operations under the lock, never wall time: no fetch, worktree add, or hook subprocess runs while the flock is held; a sleeping hook does not block a disjoint create.

# Acceptance Criteria

- No `fetch_remote`, `git worktree add`, or post-provision hook runs while the project-root flock is held during create (counted).
- A create whose hook sleeps holds the flock only for append-sized journal writes; a disjoint create proceeds meanwhile.
- A related create (same slot, agent, branch, or path) refuses as busy naming the live creator; a killed creator's journal classifies and recovers exactly as an interrupted create does today.
- A failed hook leaves the journal and a removable record; no ACTIVE row is published for it.
- The existing create, prevalidated-mutation, journal-classification and differential suites pass.

# Notes

Filed 2026-11-12 from the #1287 landing evidence (counted create ledger) and #1123's approved-direction design. The persistent incremental classification cache and compaction proposals in #1123 remain deferred, not approved. Related: #1123, #1276, #1287.
