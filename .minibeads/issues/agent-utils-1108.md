---
title: 'remove-lock-hold-starves-validate: a slow wrkslots remove holds the registry locks long enough to refuse validation admission'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-09T02:30:00.000000000+00:00
updated_at: 2026-11-09T02:30:00.000000000+00:00
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
