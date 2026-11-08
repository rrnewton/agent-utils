---
title: 'merge-queue-mid-run-intake-config: a same-head resubmission joins the run that ejected it, and a validator change reaches donor jobs at plan-intake'
status: closed
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

Implemented as designed. A same-head resubmission of a change a run
excluded applies at the next intake point once per run per change
(`resubmitted_same_head` guard in the run; the ejection stays on
record, and a second same-head resubmission waits for the next run).
At plan-intake the run reloads the config: a changed validator digest
swaps the validator donor jobs are published with and recomputes the
donor tool identity, recording a `config-reload` event naming both
digests; local validations keep the run-start validator. No new drain
verb was added: `drain-for-upgrade` already finishes in-flight work,
starts nothing new and exits for a fresh-config run, and the user
guide now points validator changes at it and at the plan-intake
reload.

Tests: `a_resubmission_at_the_ejected_head_is_taken_in_once_in_the_same_run`
(cli_intake_window, rewritten from the old same-head-wait test) and
`a_validator_change_mid_run_reaches_later_donor_jobs` (cli_donor: the
earlier job keeps the old validator and digest, a job published after
the on-disk change carries the new validator and a different digest,
and the run records `config-reload`). The donor test caught a real
half of the change the first version missed: jobs carried the new
validator but the run-start tool digest, so donors would have
mismatched every reloaded job; the tool identity is now recomputed
at reload. Focused suites pass: cli_intake_window 6/6, cli_intake
8/8, cli_drain 5/5, cli_donor 35/35.

Gate note (added at landing): the first full gate on 9ac15fe0 failed
only `rust.merge-queue.test` (gate-ledger ticket
1794139030020563692-dd16a648, failed_steps exactly that one; the
exclusive phase passed) — the old 1200 CPU-s budget of #1051. The
change was rebased onto main containing #1051 (2400 CPU-s) as
5a4af5aa (one conflict in plan_intake's new `view` argument, keeping
both sides) and `rust.merge-queue.test` was rerun alone through
gate.py --rerun: PASS, 574 cases, 572 passed, 0 failed, 657.8 CPU-s
(user 456.3 + sys 119.3 in the step profile; 503 s wall), ledger
verdict pass for 5a4af5aa6149bbddf0993cff74f569e8adfa220a.
