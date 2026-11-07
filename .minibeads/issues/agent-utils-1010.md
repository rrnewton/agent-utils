---
title: 'merge-queue-withdrawn-batches: a batch holding a withdrawn change is retried, blocks disjoint roots, and stops the run'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-07T12:00:00.000000000+00:00
updated_at: 2026-11-07T12:00:00.000000000+00:00
---

# Description

A merge-queue run holds the writer lock for its whole length, so a
`merge-queue withdraw <PR>` that arrives during a run is only recorded
(`intake::pending_withdrawals`) and applied when the run ends. Until then,
in-flight batches holding the withdrawn change are withdrawal-blind:

- `Runner::refused_again` never checks `pending_withdrawals`, so a batch
  whose validation came back refused (could-not-run) is retried up to
  `validator.refusal_max_retries` times, each retry a full validation
  whose result can never land (GitHub issue 306,
  merge-queue-withdrawn-refusal-retry).
- Nothing removes the withdrawn batch from the window, so a disjoint
  root that jumped over it stays undecidable (`rooted_decidable` waits
  for every jumped-over flight to leave the window), and when the
  withdrawn batch comes back green, `land_with` returns
  `BatchOutcome::Stop`, which ends the whole run instead of dropping
  the one batch (GitHub issue 307, merge-queue-withdrawn-root-release).

On 2026-11-07 a consuming project's queue held three green disjoint
roots for 23, 15 and 8 minutes behind a withdrawn in-flight batch, and
the operator restarted the serve daemon to apply the withdrawal.

# Design

- Add one shared primitive: at each landing-loop pass, drop every
  in-flight flight whose candidate holds a pending withdrawal. Set its
  cancel flag (which cancels its donor job), cancel any culprit-search
  probes that follow it, requeue only the non-withdrawn changes that
  were built on its tree, and set aside changes that depend on the
  withdrawn change for the rest of the run. The withdrawn changes
  themselves are not requeued: the recorded withdrawal is applied by
  the normal intake apply when the run ends.
- `refused_again` returns `None` for a flight holding a withdrawn
  change, so it is never retried or re-published.
- A withdrawn batch that still reaches a decision (refused or green)
  is decided as withdrawn and the run goes on: `land_with` returns a
  new non-stopping withdrawn outcome instead of `Stop`.
- No change to `rooted_decidable`: once the withdrawn flight leaves
  the window, roots that jumped over it become decidable at once, as
  with a held item.

# Acceptance Criteria

- A refused batch whose change is withdrawn during its first attempt
  is validated exactly once, is not deferred as refused, and its
  change is withdrawn when the run ends.
- A disjoint root that jumped over a batch withdrawn while it
  validates lands in the same run, without waiting for the withdrawn
  batch's validator to return, and the run exits normally.
- A withdrawn batch that comes back green does not land and does not
  stop the run; a batch behind it still lands in the same run.
- Withdrawing a red batch whose culprit search is probing cancels its
  probes and records no ejection for the withdrawn change.
- Existing merge-queue behaviour is otherwise unchanged: held items,
  refusals without a withdrawal, stale roots and culprit searches
  behave as before.

# Notes

Filed from GitHub issues 306 (merge-queue-withdrawn-refusal-retry) and
307 (merge-queue-withdrawn-root-release), reported 2026-11-07 by the
operator lane of a consuming project's merge queue. The two are landed
together because they share the withdrawn-flight handling above.
