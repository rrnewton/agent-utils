---
title: 'merge-queue-pre-claim-fetch-order: the pre-claim check runs before a remote donor has the candidate'
status: open
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
- In the same change, count stale-unadmitted cancellations: a
  cancelled validation record's stale reason distinguishes a lease
  that had not reached validator start, and `stats` exposes the
  count so operators watch the frequency. Queue position is
  unchanged: a requeued batch keeps priority, plan index and age
  ordering as its new job is republished fresh.

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
