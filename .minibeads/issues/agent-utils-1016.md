---
title: 'merge-queue-stale-probe: a culprit-search probe cancelled as stale counts as an inconclusive search'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-07T15:30:00.000000000+00:00
updated_at: 2026-11-07T15:30:00.000000000+00:00
---

# Description

#1013 merge-queue-stale-cancel-retry made a stale-marked donor
cancellation return a cancelled result instead of a refused one.
That path also fires for culprit-search probe jobs: when the base
moves during a red batch's search and the donor sweep cancels a
probe job as stale, the search's `record` counts the cancelled probe
as an observation that saw only missing/skipped tests, and the
search ends `Inconclusive` — a `BatchOutcome::Stop` that ends the
run with exit 3 and names no culprit.

Seen at 38908bd2 in `cli_donor`'s
`kept_stale_roots_keep_their_donor_jobs_and_decide_after_a_landing`:
passes at 98f4b65d, fails when the sweep wins the race against the
probe ("stopped: culprit search inconclusive for #2: probe at
prefix 0 of 1 observed only missing/skipped tests").

# Design

- In the culprit search's `record`, a probe whose result is a
  stale-marked cancellation (`cancelled` with a `stale:` reason)
  ends the search with a new `Culprit::Stale` verdict instead of an
  observation: the search is abandoned and the red batch is
  recomposed on the current base, like any stale batch.
- A stale-cancelled probe's "no verdict" result never counts as an
  inconclusive culprit search.
- Batch jobs keep #1013's behaviour unchanged.

# Acceptance Criteria

- `kept_stale_roots_keep_their_donor_jobs_and_decide_after_a_landing`
  passes repeatedly on the fixed tree.
- A new `cli_stale_cancel` test: a confirm-search probe donor job
  cancelled as stale mid-search abandons the search, the red batch
  is recomposed and ejected in the same run, and the run exits 0
  with no inconclusive stop.
- The whole merge-queue crate suite passes on the exact push tree.

# Notes

Found 2026-11-07 by a consuming queue's operator after swapping to
38908bd2; reproduced here at 38908bd2 (3 of 5 runs of the kept-roots
test). Regression from #1013 merge-queue-stale-cancel-retry.
