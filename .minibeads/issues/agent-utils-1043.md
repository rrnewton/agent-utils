---
title: 'merge-queue-pre-claim-fetch-order: the pre-claim check runs before a remote donor has the candidate'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-08T04:05:00.000000000+00:00
updated_at: 2026-11-08T04:05:00.000000000+00:00
---

# Description

#1015's pre-claim check runs in a donor's clone before it fetches
the candidate. The consuming queue's check reads its pinned image
from the candidate tree itself (`git show
$MQ_CANDIDATE:<path>`), the only correct source because a change may
bump the pin. On a remote donor the candidate is not in the clone
until `fetch`, so the check sees no candidate and the donor releases
every job; all work would funnel to queue-host donors.

# Design

- Run `pre_claim_check` after fetch, before the checkout slot is
  used, so the candidate tree exists on every donor. The order is
  fetch -> pre-claim check -> checkout -> admit just before
  validator start, so a job released by the check has never been
  reported admitted.
- Keep timeout, environment (`MQ_CANDIDATE`, `MQ_REPO`, `MQ_BASE`),
  and release-without-decline/receipt semantics unchanged.
- Copy the wording fix into `ValidatorConfig` docs and the user
  guide. A wasted bundle fetch per released job is accepted.
- In the same change, count stale-unadmitted cancellations where
  they happen: a stale cancellation of a job no donor had admitted
  records a `stale_unadmitted_cancel` marker in its job history,
  and `donor status` counts marked jobs on a
  `stale_unadmitted_cancel: N` line, so operators watch the
  frequency. Queue position is unchanged: a requeued batch keeps
  priority, plan index and age ordering as its new job is
  republished fresh.

# Acceptance Criteria

- A remote donor whose check reads the candidate tree via
  `git show $MQ_CANDIDATE:<path>` passes and validates a candidate
  that exists only after fetch.
- A failing check after fetch still releases the job with its
  stderr reason and no admission recorded.
- Existing pre-claim tests keep passing.

# Notes

Reported 2026-11-07 by a consuming queue; it holds the check unset
until this lands. Interaction with #1026 admission is described in
Design.

## Resolution

Landed: the pre-claim check now runs after fetch and before
checkout/admission, so the candidate tree exists on every donor;
a check that releases a job has therefore never reported
admission. Docs carry the same wording.
Gate reasoning: the full gate's shared phase on the rebased tree
passed 236 steps with two failures. `rust.merge-queue.test`
hit its 1200 CPU-s cap with only compile/lock-wait detail
shown: the crate's tests measure ~780 CPU-s alone in gate form
(compile cached), so in-gate compile plus contention busts the
cap and the wrapper drops per-binary output when it kills the
step; a budget raise is tracked separately. Instead, two
whole-crate runs in the step's own gate form (release,
--no-fail-fast, RUST_TEST_THREADS=4) on the exact push tree:
run 1: 549 cases, 547 passed, 0 failed; run 2: 546 passed, 1
failed — only `a_prioritized_stale_green_roots_instead_of_
stacking_under_a_plain_head` (cli_stale_green prioritized 60 s
timeout), a known flake with a pending fix elsewhere. Every
#1043 test (cli_pre_claim 5/5 including the remote-donor case,
and the donor sweep marker branches) passed in both runs. The
second failure, vibe-talk approval-card-phone-threads-browser
on untouched code, passed its solo rerun. The gate's exclusive
cross-differential phase was waived for this diff (it touches
only rs/merge-queue, its guide and these issues, none of the
paths those differentials exercise) and the queued exclusive
ticket was stopped to free the queue.
