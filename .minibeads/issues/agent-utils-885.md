---
title: 'wrkslots-orphaned-create-journal: an orphaned create can be aborted and never blocks unrelated creates'
status: closed
priority: 0
issue_type: bug
assignee: mu-salvage
created_at: 2026-11-05T11:42:03.624088077+00:00
updated_at: 2026-11-06T01:29:57.934729113+00:00
closed_at: 2026-11-06T01:29:57.934728973+00:00
---

# Description

P0. One orphaned `wrkslots create` blocks every later create in a consumer project, including the creation of its merge queue's validate checkouts. It also cannot be aborted through any supported command.

**State (2026-11-05, consumer host):**
- A `CREATE.<n>.<host>.<seq>.<slot>.journal` records owner null, hook_progress 1, and a coordinator_lease naming the project coordinator's own live process.
- The create was orphaned when its parent shell received SIGTERM. It had been refused mid-way ("coordinator PID … is not in the invoking process ancestry"), leaving a partial slot directory and a Git worktree registration.

**Effect:**
- Every later create refuses with "process state is indeterminate: owner=indeterminate (no owner process generation is recorded); coordinator=live". `_cmd_create` runs `_classify_create_journals`, which refuses on any indeterminate journal (py/wrkslots/cli.py ~9928-9933), before `_assert_interrupted_creates_unrelated` can say the journal concerns a different slot.
- `recover --slot … --coordinator-pid <coordinator>` and `--abort-create` refuse with the same text.

# Design

1. **Supported abort.** For an owner-less create journal whose coordinator_lease is the invoking coordinator itself (live, and proven to be the caller), when no create is running (the journal lock is free, or held by no live process): roll the partial slot back according to the journal's recorded phase.
   - Fence the partial directory, prune the Git worktree registration, and leave no registry row.
   - Record it with an event kind the pinned consumer wrkslots already understands.
2. **Unrelated creates proceed.** A create of a different slot runs the unrelatedness check first. An indeterminate create journal for another slot, with disjoint paths, does not block it. Keep refusing when the requested slot or paths overlap the journal's.
3. Both editions.
4. **Compatibility.** The post-abort state is readable by the consumer's pinned wrkslots (agent-utils f79789533dc0), and that version's create-journal classification passes on it.

5. **Lead's analysis (2026-11-05, 06:46 AM ET):**
   - **Minimal classification fix:** when owner is None, `_classify_create_journal` and `_assert_create_journal_classifications_stable` take the owner state from the coordinator lease. This is safe because create and every mutating classifier hold `_mutation_locks`.
   - **Deadlock:** a validate-batch seal combined with an unclassifiable create journal deadlocks `recover`, because `_validate_batch_seal_paired_finish` classifies all create journals first. When `--slot` names a create journal that overlaps no seal target, recover it before the seal.
   - **Hooks done:** after hooks have completed, the right recovery is forward (publish the row). Abort needs a verified clean worktree removal that includes initialized submodules.

6. **Journal gone, history names it (2026-11-05, 06:53 AM ET).** A hand repair that moved the create journal aside made every recover refuse with "recovery journal … is missing while append-only history still names it" (rc 3). Provide a supported way to make such an interrupted create terminal, or at least a runbook in the userguide.
7. **Landing order:** the minimal classification change (5, first bullet) lands first and alone, with a tight gate. The deadlock, forward/abort and missing-journal items follow as separate landings.

8. **Unscoped vs scoped recover (2026-11-05, 09:13 AM ET).** On the same state, unscoped `recover --coordinator-pid P` refused ("validation-batch seal recovery retained 1 target (sealed target absent while ACTIVE row remains)"), while `recover --slot <slot> --coordinator-pid P` resolved it (finish recovered; seal resolved). Make the unscoped path do the same, or name the `--slot` to use.

# Acceptance Criteria

- Test: SIGTERM a create's parent mid-create, leaving an owner-less create journal; then a create of an unrelated slot succeeds.
- Test: the coordinator's `recover --abort-create` rolls the partial slot back cleanly (directory fenced, registration pruned, no row), and is refused while a create still holds the journal lock.
- Test: the pinned f79789533dc0 code classifies the post-abort state cleanly.
- Landed alone, with the SHA sent to the lead at once.

# Notes

Reported by the lead on 2026-11-05 at 06:41 AM ET (P0). Part (a), 5c4796cc, resolved the consumer's outage at 09:13 AM ET: the create was recovered forward and validations resumed. The remaining items are P0 for recurrence. Assigned to mu-salvage, which pauses #881 for it.
