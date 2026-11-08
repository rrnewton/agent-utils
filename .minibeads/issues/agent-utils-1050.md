---
title: 'merge-queue-mid-run-intake-config: a same-head resubmission joins the run that ejected it, and a validator change reaches donor jobs at plan-intake'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-08T08:20:00.000000000+00:00
updated_at: 2026-11-08T08:20:00.000000000+00:00
---

# Description

Two run-lifecycle gaps keep a long run from reacting to what its
operator does while it is open.

1. A change a run ejected can only rejoin that run at a new head.
   A resubmission at the very head it was ejected at waits in
   pending intake ("excluded this run") until the run ends, even
   when the window has free slots and the operator resubmitted it
   deliberately — typically because the thing that made it fail
   (the validator itself) has been fixed, not the change. Applying
   it today needs a restart that cancels healthy validations.

2. A validator change never applies while mid-run intake keeps a
   run alive. Config is loaded when the run starts, so donor jobs
   published hours later still carry the old validator, and the
   run must be ended by hand before the new validator is used.

# Design

- Mid-run intake: a recorded `submit` at the same head a change
  was excluded at in this run (ejected or set aside), for a change
  not in flight, applies at the run's next intake point, once per
  run per change. The exclusion is dropped, the change is planned
  again in the same run, and the ejection stays in the run report
  so a later culprit search still sees it. A second same-head
  resubmission in the same run keeps waiting. New-head
  resubmissions keep working exactly as today. This is a
  deliberate behaviour change: the pending-intake wait text and
  the test that asserted same-head requests wait for the run end
  are updated to say a same-head resubmission is taken in once
  per run.
- Config at plan-intake: when a run takes intake in, it reloads
  the queue config; if the validator's digest differs from the
  one the run started with, donor jobs published from then on
  carry the reloaded validator, and the run records a
  `config-reload` event naming both digests. Jobs are published
  per batch, so one batch never mixes two validators. Local
  (non-donor) validations keep the validator the run started
  with; ending the run remains the way to change those.
- Operator drain already exists as `drain-for-upgrade` (finish
  the validations in flight, start no new one, exit so the next
  run loads fresh config), so no second drain verb is added; the
  user guide points validator changes at it and at the
  plan-intake reload above.

# Acceptance Criteria

- A change ejected in a run, resubmitted at the same head while
  another batch is in flight, is validated again in the same run
  (and lands if the validator now passes it).
- A second same-head resubmission of a change excluded again in
  the same run is not taken in again; it waits for the next run.
- A donor job published after the validator config changes on
  disk mid-run carries the new validator (its recorded digest
  differs from a job published before the change), and the run
  report records the reload; no batch mixes validators.
- The existing drain tests keep passing (finish in flight, start
  nothing new, exit), and the guide names `drain-for-upgrade` as
  the operator drain for config changes.
- Existing intake tests keep passing, except the one asserting
  the old same-head wait, which is rewritten for the new rule.

# Notes

## Resolution
