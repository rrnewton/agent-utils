---
title: 'merge-queue-landed-intake-cleanup: recorded requests survive the landing that moots them, and a withdrawn flight late result is logged as lost'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-09T00:22:13.000000000+00:00
updated_at: 2026-11-09T05:07:26.000000000+00:00
---

# Description

Two leftovers around a landing, both seen in an operator's runs:

1. Recorded requests are not cleaned up when their change lands. A
   submit for a change in flight stays in `intake/pending` while the
   batch validates; the batch lands, and the next apply (in that run
   or at its end) carries the stale submit out anyway: the landed
   change is queued again at its pre-landing head. One run landed a
   change at 13:42:48 and applied a same-head submit recorded at
   13:24:54 two seconds later, re-queueing the just-landed change.
   The apply path has the same gap in general: `ops::submit` always
   writes a `Queued` item and clears `landed_as`, so any recorded
   submit applied after its change landed resurrects it, including
   a submit naming the landed (rebased) copy itself. Withdrawals
   already refuse politely after a landing; submits do not.

2. A validation result that arrives after its flight was dropped
   is logged as an unexplained loss. `drop_withdrawn_flights`
   cancels a withdrawn change's validation and takes the flight out
   of the window without waiting for its result (by design: a root
   it releases must not wait); when the cancelled validation does
   report, the run loop can only say "a validation result arrived
   for unknown flight 0" (kind `result-lost`). One run logged
   exactly that one second after applying the withdrawal that
   cancelled the flight. The result's origin is known and it should
   be discarded with its reason, leaving `result-lost` for a result
   the run truly never knew.

# Design

- When a run lands a batch, it drops the landed changes'
  still-waiting intake entries (all recorded before the landing by
  construction) and logs one intake event per change naming what
  was dropped. A new `intake::drop_pending_for(state, pr)` removes
  the pending files and returns what they asked for; the landing
  path in run.rs calls it for each landed change under the writer
  lock it already holds. Requests recorded after the landing are
  untouched, and a `mark-exact` requirement is never dropped: it
  records that another repository pins the landed head, and
  applying it after the landing is how that stays on record (a
  peer run that lands the change mid-cycle applies it at its end,
  as cli_peers pins). This also closes the #1050 same-head path for landed
  changes: there is nothing left for a later intake round in the
  same run to take in again.
- At apply time (`intake::perform`, Submit arm), a submit whose
  item is already `Landed` is dropped with a note ("dropped submit
  #N: #N already landed as <sha>") instead of re-queueing: an `Ok`
  applied line, not a refusal, since nothing was wrong when it was
  recorded. This covers a landing by a different holder between
  record and apply, including a submit naming the pre-landing head
  or the `landed_as` copy. A recorded submit is stale intent by
  construction; deliberately queueing a landed change again
  remains a fresh direct `submit` once the lock is free, which
  reads current state.
- The run remembers flights it dropped without consuming their
  results: `drop_withdrawn_flights` records each dropped flight id
  with its reason (withdrawn, built on a withdrawn batch, probe of
  a dropped batch) in a Runner map. At the result-arrival point a
  result for a dropped flight is discarded with an event naming
  the flight and the reason (kind `result-discarded`); a result
  for a flight the run never knew keeps today's `result-lost`
  event. Audited: `discard`, `cut_after` and `cancel_rooted_where`
  all drain the result channel before returning, and
  `discard_detached` runs only at a stop decision after which the
  loop has returned, so `drop_withdrawn_flights` is the sole
  source of late results and the only place that records them.
- Backward compatible: no state or protocol change; the notes are
  new event/intake lines only. User guide: the intake section
  gains the landing-drop rule.

# Acceptance Criteria

- A same-head resubmission recorded while a batch is in flight,
  then the batch lands: the change stays landed, is not validated
  or queued again in that run or at its end, and the drop is noted
  in the run's events/intake lines.
- A recorded submit applied after its change landed is dropped
  with a note and the item stays landed.
- A withdraw cancels an in-flight flight and its result then
  arrives: a `result-discarded` note names the flight and the
  withdrawal, no `result-lost` line appears, and no verdict is
  taken from the result.
- Existing intake, withdrawn-batch and run suites keep passing;
  the whole merge-queue crate stays clean.

# Notes

Evidence from an operator's queue: run 20261108T115231Z (a change
landed at 13:42:48; a submit recorded at 13:24:54 was applied at
13:42:50, re-queueing it) and run 20261108T160845Z (a withdrawal
applied at 16:16:20; `result-lost` for flight 0 at 16:16:21).
Consumers are referred to generically here, per repository
convention. Second in the intake chain after #1087
merge-queue-stranded-intake.

## Resolution

Landed on main as 8f44249dd786b9d6a23cd29bfa3c985fe317c0ff (gated commit 7ad4fad8, tree-identical).

`record_landed` now drops each landed change's still-waiting
intake entries through `intake::drop_pending_for`, one intake
event naming what was dropped, and a recorded submit whose item
is already Landed drops at apply with "already landed as"
instead of re-queueing it. A `mark-exact` requirement is exempt
from the landing drop — the first gate run caught its deletion
through cli_peers, whose peer-lands-mid-cycle flow applies the
requirement after the landing — so it alone survives to its
apply. Flights dropped by `drop_withdrawn_flights` are recorded
with their reasons, and a result arriving for one is discarded
as `result-discarded` (flight and reason named, no verdict
taken); a result for a flight the run never knew still logs
`result-lost`. The guide's run-directory section now divides the
three discard kinds with #1100's `flight-discarded`: discard
paths record at the drop, only a withdrawn flight can still
report afterwards, and no flight takes two kinds. Tests: a
resubmission recorded in flight dropped at landing
(cli_intake_window), a recorded submit for a landed change
dropped at the next writer (cli_intake), a late result for a
withdrawn flight discarded with its reason
(cli_withdrawn_batches), all written failing first, plus the
existing cli_peers mark-exact case as the exemption's guard.
Gate: first full gate on 25226aaf (239 steps) failed only
rust.merge-queue.test on that mark-exact case; the fix was
treated as an amendment — rebased onto main with #1100, then
one exclusive rerun ticket on the exact landed tree
(rust.merge-queue.test whole crate, rust-clippy, rust format,
client-names; 6 steps incl. deps) passed, documented in the
gate ledger as rerun_passed for 7ad4fad8.
