---
title: 'remove-lock-hold-starves-validate: a slow wrkslots remove holds the registry locks long enough to refuse validation admission'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-09T02:30:00.000000000+00:00
updated_at: 2026-11-09T18:00:00.000000000+00:00
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
- Required tests (the lead): a process that enters the slot between the unlocked evidence pass and the fence makes the remove refuse (and the slot is restored); a concurrent `adopt` (or create binding the slot) during the unlocked pass makes the remove refuse under the lock.

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

Decision (the lead, 2026-11-08): keep the cheap registry-state checks under the lock, and move EVERY /proc scan out of it. What each guard protects against:

| Guard | Where | Protects against |
|---|---|---|
| Registry re-check: the record's generation and owner unchanged since the unlocked pass, and no new claim (no adopt, transfer, create, hold, release, heartbeat renewal or handoff since then) | under the registry locks, immediately before the fence | a registry change that binds the slot again or changes who owns it, between the evidence pass and the fence; the lock makes the re-check and the fence one atomic step for every other wrkslots command |
| Unlocked use scans (today's precondition scans, moved out of the lock) | before the locks | nothing the later guards miss; they keep an early refusal with today's messages, before any fence |
| Path fence (rename to a random name under a private parent) | under the locks | a process reaching the tree by name after the fence: the canonical path is gone and the fenced name cannot be learned |
| After-fence use scan with #1102 re-listing | unlocked, after the fence, before deletion; a refusal renames the fence back | any process that holds a reference into the tree (it held it before the fence, inherited it from a holder, or appeared during the scan), found by the fenced path and by device and inode |

Process safety therefore rests on the fence plus the after-fence scan; registry safety rests on the locked re-check plus the fence. The registry locks never protected process use.

Open point for the lead: image-backed slots. Their fence remounts the image, and a process still inside the OLD mount is invisible to the after-fence identity scan, so the table's last row does not cover them. Today a lsof check (`before_image_fence`) runs right before that rename, under the locks. Moving it out of the lock reopens that window for image slots only. Proposal: keep it under the lock for image-backed slots and measure its cost there; or, if every scan must leave the lock, re-run it after the fence against the old mount's device before deletion. Needs the lead's choice.


Image-backed slots (the lead, 2026-11-08, answering the open point): option (a). The `before_image_fence` lsof check stays under the registry locks. Neither a mount-generation re-check nor a re-run after the fence can catch a NEW opener that arrives between an unlocked lsof and the fence, and image slots are the common case on both hosts, so correctness wins. Its hold gets its own line in the #1108 measurements. If it dominates under load, the follow-up must first stop new entrants and only then run an unlocked lsof (fence first, then lsof plus the remount); a generation check is not enough.

Implementation, Python (cl-wrkid, 2026-11-08):

- Unlocked pass: `_remove_unlocked_use_pass`. Before the locks, an agent removal (a remove-agent-batch item, or an ordinary remove run as a batch of one) reads its record without the lock. It checks the shared lsof evidence and runs the item's single fresh /proc scan (`_AgentRemoveBatchContext.observe_unlocked`). A refusal is raised where the locked use check used to refuse, so messages and their order do not change.
  - The scan compares owner cgroups only when the owner is not live. A live owner has either consented or released, in which case the locked check drops the same comparison, or the locked pass refuses the removal anyway.
- Under the locks: `_assert_unlocked_record_current` runs right after the generation check and refuses when the record differs in any field from the one the unlocked pass read. The message names the changed fields. This catches an adopt, transfer, heartbeat, release, hold-bound change, or a record that appeared.
  - The locks are then held through the fence, so the re-check and the fence form one step.
  - Both pre-fence use checks (the precondition check and the one in `_begin_or_resume_path_fence`) now only re-read that evidence: the lsof coverage and the slot directory identity. No /proc scan runs under the locks on this path.
  - `fresh_process_scans` in remove-agent-batch output is now one per item instead of two.
- After the fence: unchanged and decisive. The fenced scan runs unlocked, with #1102 re-listing; a refusal rolls the fence back.
- Still under the locks:
  - image-backed slots' `before_image_fence` lsof check (the decision above);
  - the ordinary checks of removals that do not run as an agent batch (image-backed single removes, `--no-lock-budget`, hosts without lsof, validation and private-cleanup removals);
  - the #993 unattributed-pane case's argv/environ naming scan, which depends on the registered-liveness answer taken under the lock. Moving it needs that answer before the lock too; follow-up if the measurement shows it matters.
- Tests (py/wrkslots/tests/test_lifecycle.py):
  - `test_agent_remove_runs_no_process_scan_under_the_registry_locks`: every scanner (shared lsof census, fresh scan, fenced scan, lsof, /proc listings, process snapshot) is called while no `_mutation_locks` hold is open, and the fresh scan runs exactly once. The lock-wait bound follows from this: the hold no longer contains a scan, and its size under load is the daytime measurement.
  - `test_agent_remove_refuses_a_process_that_enters_after_its_unlocked_scan` (required test 1): a process starts in the slot under the locks, after the unlocked scan. The after-fence scan names it at the fenced path, the fence is rolled back, and the slot, record and journals are as before.
  - `test_agent_remove_refuses_an_adopt_during_its_unlocked_scan` (required test 2): an ownerless, expired slot is adopted after the unlocked pass; remove refuses under the locks with `changed: heartbeat_at, owner`.

Rust parity gap (recorded, not mirrored): `remove_write.rs` runs its one pre-fence `assert_slot_unused` under its mutation locks (~1869). Moving it out safely needs what Python has after the fence, a rollback of the fence when the after-fence scan refuses. Rust instead leaves the FINISH journal for `wrkslots recover`, so the race this change tolerates would turn into a recovery-required state there. Rust remove is still fixture-only (`require_fixture`) and refuses image slots and owner consent. For the Rust wrkslots owner via the coordinator: add the after-fence rollback, then move the scan before the locks with a record re-check, as in Python.

Remaining before closing: the daytime load measurement of the lock hold with this change (image-slot `before_image_fence` hold measured separately), and validation admission's shared read lock (second design option), if the measurement still shows admission waiting.

Measurement 2, with the change (cl-wrkid, 2026-11-09 daytime, main at 2ae788cf, no added load, host load 41-93, no gate running). Method as in measurement 1: one Python `wrkslots remove` of a verified-dead agent (worktree) slot on the remove-differential fixture, with `_locked` and the phases wrapped in timers; three runs, the third with every scan made 5 s slower through the census churn seam. Logs: /tmp/cl-wrkid-1108b/ on devbig030.

| phase | run 1 | run 2 | run 3 (+5 s per scan) | locks |
|---|---|---|---|---|
| shared lsof census | 8.88 s | 8.27 s | 8.86 s | none |
| unlocked use pass (fresh scan) | 3.57 s | 3.56 s | 8.73 s | none |
| registry locks held (ACTIVE, ACTIVE.<machine>.json) | **2.03 s** | **1.99 s** | **2.05 s** | held |
| of which the locked whole-record re-check (`_assert_unlocked_record_current`) | <0.01 s | <0.01 s | <0.01 s | held |
| of which the path fence | 0.54 s | 0.54 s | 0.51 s | held |
| after-fence scan and deletion (`_finish_remove_paths`) | 4.92 s | 4.96 s | 9.99 s | none |
| final registry update | 0.03 s | 0.03 s | 0.04 s | held |

The worktree-slot lock hold is now about 2 s and does not grow with scan time. Before the change it was 8.5 s, growing to 18.7 s with the same slowdown (measurement 1). The rest of the hold is the cheap checks and the fence.

Image-backed slots, measured separately: their `before_image_fence` check (`observe_image_before_fence`, which runs `_observe_slot_use_once`: lsof +D plus the direct /proc scan) stays under the locks, per the lead. That call, timed alone five times on a one-file scratch directory: 11.64, 11.52, 12.16, 11.54, 11.44 s at load 42-93, about 8 s lsof and 3.5 s direct scan, matching the remove runs' census and scan times. This is a lower bound for a real image slot, whose lsof +D also walks every file of the slot's checkouts. No image slot was created: mounting one needs root, which this lane did not use. So an image-slot removal holds the registry locks for about 2 s plus 11.5 s or more, and that grows with host load as lsof does. It dominates the hold, about 85% at this load.

Proposal (for the lead): the hold is solved for worktree slots, and image slots, the common case on both hosts, still hold the locks for the whole lsof. By the lead's rule ("if it dominates under load, the follow-up must first stop new entrants and only then run an unlocked lsof"), that follow-up is due: under the locks, fence first (stop new entrants by path) and release; then run lsof against the old mount, plus the remount, outside the locks, with the after-fence checks staying decisive. Two ways to track it: (a) close #1108 with this measurement as its Resolution, for worktree slots and the measurements, and file the image follow-up as a new P1 issue; or (b) keep #1108 open for it. Recommendation: (a), since the follow-up is a separate design (it changes the image fence's order and needs its own safety argument); #1108's validation-admission read-lock option can be dropped unless admission still waits once both are in.

## Resolution

Closed by the lead's decision (2026-11-09, relayed by the coordinator), with measurement 2 above as the result. The change (be8b3a8e) moved remove's slot-use scans before the registry locks and keeps a cheap locked re-check under them. A worktree-slot removal now holds the locks for about 2 s (2.03, 1.99, 2.05 s), flat when every scan is 5 s slower; before, it was 8.5 s growing to 18.7 s. The locked whole-record re-check takes under 10 ms.

Not solved here: image-backed slots. Their `before_image_fence` lsof check stays under the locks, by the lead's earlier choice, and dominates their hold: 11.4-12.2 s at load 42-93, a lower bound measured on a scratch directory. The fence-first follow-up is #1142 image-remove-fence-first. The validation-admission shared read lock, the second design option, is dropped for now (the lead).

