---
title: 'merge-queue-stale-cancel-retry: a donor job cancelled as stale is retried as the identical candidate instead of recomposed on the current base'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-07T13:50:00.000000000+00:00
updated_at: 2026-11-07T14:20:00.000000000+00:00
---

# Description

After a landing moves the base inside a run, the donor jobs of batches
built on the previous base are cancelled as stale: by the run's own
`prune_donor_jobs` and by the claim-time `Store::sweep_stale` every
donor claim runs ("stale: the base moved to ..., which its candidate
does not contain"). `Runner::donor_validate` maps any such outside
cancellation to a synthetic admission refusal (`refused: true`), so
`refused_again` republishes the identical stale candidate up to
`refusal_max_retries` times (about 2 s apart). Each republication is
cancelled as stale again within seconds, and when the retries run out
the batch is deferred still holding its stale candidate: it is never
recomposed on the current base during the run.

Seen on 2026-11-07 in a consuming project's queue (before
#1010 merge-queue-withdrawn-batches): three rebuilt roots on the
previous base each looped through three identical retries after the
base moved, while the run already knew the new base.

# Design

- A stale-marked cancellation is not a refusal; it is the stale-root
  signal arriving through the donor channel. In `donor_validate`'s
  outside-cancellation handling, when the cancellation is stale (the
  history names the base move, or the candidate no longer contains
  the current base), return a cancelled result instead of a refused
  one. `refused_again` then does not retry it, and the flight falls
  into the run's existing stale machinery, which requeues its changes
  and rebuilds the candidate on the current base.
- Non-stale outside cancellations (an operator's cancel, the
  could-not-run bound) keep today's refused behaviour.
- The claim-time sweep keeps sweeping queued stale jobs (including
  queued rooted jobs under `keep_stale_roots`, per #915): with the
  run recomposing instead of republishing, the sweep and the run's
  stale path now agree, and no identical stale candidate is ever
  republished.

# Acceptance Criteria

- A donor job cancelled as stale after the base moves mid-run is
  recomposed once on the new base in the same run: the replacement
  job's candidate descends from the new base, and the change lands.
- No donor job is ever published twice with the same stale commit,
  and no `validation-refused` retry is recorded for a stale
  cancellation, however often donors claim meanwhile.
- An operator's outside cancellation still refuses the batch, with
  the run going on (existing behaviour, covered by
  `an_outside_cancellation_refuses_the_batch_and_the_run_goes_on`).

# Notes

Reported 2026-11-07 by the operator lane of a consuming project's
merge queue and traced there; verified against this tree. Not covered
by #1010 merge-queue-withdrawn-batches, whose `refused_again` guard is
withdrawn-only.

## Resolution

Landed: a stale-marked outside cancellation in `donor_validate`
returns a cancelled result (marked stale in its reason) instead of a
refused one, so `refused_again` never republishes the identical stale
candidate; at its decision the batch is recomposed on the current
base (`stale-cancelled`). Covered by
`rs/merge-queue/tests/cli_stale_cancel.rs`: a stale outside
cancellation is recomposed and lands with no refused validation
(fails before the fix), and a stale-cancelled kept root is recomposed
on the new base with exactly one job for the stale candidate.
