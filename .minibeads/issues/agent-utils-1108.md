---
title: 'remove-lock-hold-starves-validate: a slow wrkslots remove holds the registry locks long enough to refuse validation admission'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-09T02:30:00.000000000+00:00
updated_at: 2026-11-09T02:42:29.000000000+00:00
assignee: cl-wrkid
---

# Description

On a consumer host with 77 slots at high load (Python edition, agent-utils a0eeb67d), a merge-queue validation at 2026-11-08 9:24 PM ET was refused:

> validate-run: REFUSED: prospective validation slot/path authority could not be proved after 147.025s (17.985s classifying, 129.040s waiting for registry locks): wrkslots could not classify create jour...

At the same time an operator was running `wrkslots remove` on a verified-dead slot; each removal takes 4+ minutes on that host. One removal held the registry locks long enough to keep validation admission waiting for 129 s, and the job was handed back (exit 75), costing about 6 minutes. The queue recorded it correctly as host scope, but every removal on a busy host can now cost a validation slot.

# Design

Two complementary options; do whichever the evidence supports, or both:

- **Shorten remove's lock hold.** Do the slow work (the process/path census, salvage bundling, size accounting) BEFORE taking the registry locks, then re-verify cheaply under them (generation, identity and fence checks) and refuse or retry if anything changed. The removal proof must stay as strong as today; only the expensive evidence gathering moves out of the critical section.
- **Let validation admission classify under a shared read lock.** Create-journal classification only reads registry state, so it can share a read lock with other readers and wait only for writers' short critical sections.

Measure first: time spent under the locks per remove phase, on a loaded host or with a forced slow census.

Sequence after #1102 wrkslots-census-churn-tolerant's vanished-PID half, which removes part of remove's census time anyway.

# Acceptance Criteria

- A test (or forced-slow-census fixture) shows validation admission proceeding while a remove does its slow evidence gathering, with lock-wait time under a stated bound.
- Remove still refuses when the slot changes between the unlocked evidence pass and the locked re-check (test).
- Both editions, or the parity gap recorded.

# Notes

Reported by the lead (devbig014), 2026-11-08, from a consumer's merge queue. The lead had a read-lock idea parked; this is its first concrete cost. Related: #1086 validate-run-authority-budget, #1098 wrkslots-classify-batch-seals, #1102.

Measurement 1 (cl-wrkid, 2026-11-08 evening, no added load): one Python `wrkslots remove` of a verified-dead agent slot on the remove-differential fixture (py/wrkslots/tests/test_remove_write_rust.py `_fixture`), with `_locked` and the remove phases wrapped in timers.

| t (s) | phase | locks |
|---|---|---|
| 0.0-7.6 | `_capture_agent_remove_batch_census` (shared lsof census) | none |
| 8.05-16.60 | registry locks (ACTIVE, ACTIVE.<machine>.json) held: 8.5 s | held |
| 8.2 | `_agent_batch_fresh_use` (full /proc scan for the slot) 3.3 s | held |
| 12.8 | `_begin_or_resume_path_fence` 3.8 s, which runs a second full `_agent_batch_fresh_use` scan, 3.3 s | held |
| 16.6 | `_finish_remove_paths`: `_agent_batch_fenced_use` scan 3.8 s, then deletion | none (#894) |
| ~21.3 | final registry update | held ~0.03 s |

With every scan made 5 s slower (the census churn test seam running `sleep 5`), the lock hold grows from 8.5 s to 18.7 s: it is about two full-host /proc scans. On the consumer host a scan at load 100-200 plausibly takes a minute or more, which fits 4+ minute removals and a 129 s admission wait. Next: measure the same phases by day under gate-like load, then move the first scan out of the critical section with a cheap re-verify under the lock (the unlocked after-fence scan, with #1102's re-listing, already catches a process that appeared meanwhile), and give admission's classification a shared read lock.

Design (cl-wrkid, from reading py/wrkslots/cli.py; implementation follows #1102 part 2, per the lead's order):

- Where the two locked scans come from: scan 1 is remove's precondition slot-use check in `_cmd_remove_in_scope`; scan 2 is `_reclaim_preconditions`, called again inside `_begin_or_resume_path_fence` just before the rename. Both run inside one hold of the registry locks.
- The registry locks do not protect against process use: an agent does not take wrkslots locks to `cd` into a slot, so a scan is equally valid inside or outside them. What matters is (a) the registry record not changing between the checks and the fence, which the locks do protect, and (b) the gap between the last use check and deletion.
- Remove's decisive use check is already the scan AFTER the path fence, which runs without the registry locks (#894), and a refusal there rolls the fence back (`_rename_slot_path(config, fenced, original)`), restoring the slot. A process that started using the slot before the fence still holds the fenced tree and is found there (with #1102 it is found even if it appeared during that scan); one that only knows the path cannot enter after the rename.
- Change: run the slot-use scans before taking the registry locks, as an early refusal with today's messages. Under the locks, re-verify only what the locks protect (record, generation, owner and handoff evidence, journals, seals: the existing cheap checks) and establish the fence and its journal; then release. The after-fence scan and deletion stay unlocked and decisive. Expected lock hold: milliseconds plus the cheap checks, independent of host load.
- Keep under the locks: the image-slot pre-fence check (`before_image_fence`), because the fence remounts the image and the after-fence identity scan cannot match a process still in the old mount; it is lsof-only, and its cost gets its own measurement.
- Admission's create-journal classification under a shared read lock is complementary (it helps readers overlap), but alone it would still wait for a long exclusive hold, so it comes second.
- Tests: validation admission proceeds within a stated wait bound while a remove's scans are slowed by the census churn seam (`sleep`); a process that starts using the slot after the unlocked pre-scan is found by the after-fence scan and the fence rolls back (forced churn); unchanged refusals when the record changes between the unlocked pass and the locked re-check. Rust: `remove_write.rs` runs `assert_slot_unused` before (~1762) and after (~2094) its fence; whether its first call is inside its locks gets checked when implementing, and mirrored or recorded.

Why the removal proof stays as strong with no /proc scan under the locks (cl-wrkid). The danger is a process P using the slot when its tree is deleted. Today's sequence is: use scans (locked) -> path fence, a rename to a fenced name with a random component under a private parent (locked) -> use scan after the fence (unlocked; a refusal renames the fence back and restores the slot) -> deletion (unlocked). Take any P that uses the tree when deletion starts:

1. P held a reference into the tree before the rename (working directory, root, open descriptor, mapping, mount). A rename keeps every inode, so P's reference now points into the fenced tree. The after-fence scan matches it by the fenced path and by device and inode (`_agent_batch_fenced_use`, `in_tree`), refuses, and the fence is rolled back. This holds whether the pre-fence scan ran inside the locks or not.
2. P got its reference after the rename. Through the canonical name it cannot (the path no longer exists). Through the fenced name it cannot learn it. Through inheritance it can only descend from a holder of case 1 or 3, which is itself found. Passing a descriptor over a socket to an unrelated process stays outside the model, exactly as today.
3. P appeared while the after-fence scan ran, for example a child forked by a holder that then exited. With #1102 that scan lists /proc again and reads every new generation, so P is read; two listings that add no new user close the scan.
4. P started using the tree after the after-fence scan closed. Unchanged from today: that scan was already the last check before deletion.

The locked pre-fence scans added nothing to cases 1-4. They only made a refusal happen before the fence instead of after it. An unlocked pre-fence scan keeps that early refusal. In the rare race where P enters the slot by path between the unlocked scan and the rename, P is case 1: found after the fence, and the slot is restored. That is a later refusal, not a weaker one.

What the locks must still protect is the registry record, not process use: that no adopt, transfer, heartbeat renewal, release, hold or handoff changed the slot between the evidence pass and the fence. Those re-checks are cheap (record digest and generation, heartbeat stamp, hold and handoff identity, journals and seals) and stay under the locks immediately before the fence. A revived owner is a process using the slot (case 1 or 3), or it renews the heartbeat, which changes the record that is re-checked. So "one cheap re-check" means these registry re-checks; no /proc scan is needed under the locks.

One exception stays locked: an image-backed slot. Its fence remounts the image, and a process still inside the old mount is invisible to the after-fence identity scan, so case 1 does not hold there. The lsof-only check right before that rename (`before_image_fence`) stays under the locks, as today.

