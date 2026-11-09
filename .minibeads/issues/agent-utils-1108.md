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

