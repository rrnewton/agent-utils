---
title: 'merge-queue-leased-unadmitted-root: a leased rooted job counts as started before its donor admits it'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-07T22:05:00.000000000+00:00
updated_at: 2026-11-07T22:05:00.000000000+00:00
---

# Description

A rooted donor job is exempt from the stale-base sweeps as soon as a
donor leases it, but the lease is only a claim: the donor may then
wait on tool-materialization or admission locks for minutes before
any validation is admitted. If the base moves in that window, the
run's stale-root handling treats the job as started and keeps it, so
the validator later spends the slot refusing it as STALE-BASE
instead of the job being cancelled with no strike and the batch
recomposed on the current base.

Seen on 2026-11-07 in a consuming queue at 98f4b65d (run
20261107T165537Z): a rooted job was dispatched on the old base, the
base moved 3 s later, the job sat "leased to local" for about 2 min
on tool locks, and the donor slot was wasted on the stale refusal.

# Design

- A leased job counts as unstarted until the donor reports
  admission. The donor records the transition when its validation
  is actually admitted (to be located precisely in
  `Worker::validate`: after the tool-identity/digest/pre-claim
  checks and when execution is about to run), and the queue-side
  `was_started`/`root_unstarted` answers, the claim-time stale
  sweep, and run-side pruning all treat leased-but-unadmitted as
  unstarted. The field must default to not admitted so records
  written before this change parse.
- A rooted job a donor has admitted keeps today's kept behaviour:
  green is requeued and red is decided.
- Fallback if an admission report proves impractical: the donor
  re-checks that the job candidate still contains the current base
  immediately before admission and gives the job back when it does
  not.

# Acceptance Criteria

- A rooted job that is leased but not admitted when the base moves
  is cancelled with no could-not-run strike, and its change is
  recomposed on the new base in the same run, with no STALE-BASE
  refusal validation for the old candidate.
- A rooted job whose donor has admitted it when the base moves is
  kept and decided as today.
- Records written before the admission field existed count as not
  admitted until a donor reports otherwise.

# Notes

From a consuming queue's backlog at 98f4b65d on 2026-11-07
(the 12:24 ET backlog entry). Owner `mu-mqwd` after
pre_claim_check.
