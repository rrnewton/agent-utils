---
title: 'merge-queue-stranded-intake: a run past its intake window drains while a recorded submission waits'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-08T20:00:00.000000000+00:00
updated_at: 2026-11-09T01:45:48.000000000+00:00
---

# Description

A run takes recorded requests in only for its first two hours
(`INTAKE_FOR` in run.rs). After that it just drains: no intake is
taken however long the remaining batches run. A submission recorded
after the window closed therefore waits in pending intake for the
whole drain, even when the run has free validation slots and only
one batch left in flight. One operator run stopped taking intake at
13:52Z and drained a single long batch while a new-head resubmission
recorded at 14:04Z sat in intake/pending for that batch's whole
remaining run: the queue was effectively idle for one change while
capacity stood free.

# Design

The two-hour cutoff stays: it is the boundary that keeps a run from
living forever on the config and tool answers it started with, and
the next run still reloads both. The rule changes only at the tail
of a run. When a run is past its intake window, has pending intake
recorded, and has exactly one batch left in flight, it takes that
intake in once more, planned on top of the in-flight batch exactly
as a free-window intake is today, and then takes no further intake
in that run. The extension is bounded by one batch validation, and
nothing else changes: before the cutoff intake works as today, a
run with more than one batch in flight drains as today, and a run
with no pending intake ends as today. No new configuration and no
protocol change, so mixed versions are unaffected: an older binary
simply drains as it always has.

Testability (decided while implementing): the window length becomes
`RunOptions::intake_for` (`Option<Duration>`, `INTAKE_FOR` when
unset), plumbed from a new `serve --intake-for-secs` flag exactly as
`intake_poll` is plumbed from `--poll-ms`; the one-shot `run` keeps
the default. An environment-variable override was rejected: every
timing knob on this binary is a serve flag funnelled through
`ServeOptions` into `RunOptions`, while its environment variables
name state and identity only, and a duration read from the ambient
environment of a long-running daemon would be invisible in `--help`
and the heartbeat. The flag is additive and defaults to today's two
hours, so existing invocations and service units are unaffected.

# Acceptance Criteria

- A new-head resubmission recorded after the intake window closed,
  while exactly one long batch is in flight, is validated in that
  run alongside the draining batch instead of waiting for the next
  run.
- A second submission recorded after that one extra intake round is
  not taken in; it waits for the next run.
- Intake before the window closes, and draining with more than one
  batch in flight, behave exactly as today.
- A maintained test, "late resubmission while one long batch
  drains", covers the first two criteria, and the existing intake
  suites keep passing.

# Notes

## Resolution

Landed on main as a79dfc4586af577ddfd1ca6adab6a741acb71002 (gated commit 092d7072, replayed).

Past its intake window a run now takes recorded requests in once
more when exactly one batch is left in flight: the ordinary
intake round at the single `takes_intake` call site runs with
`late_intake_round` (window closed, one flight — probes count, so
a culprit search waits out — round unspent), the spend is marked
only when the pending guard passes and the round is about to run,
and the log says "past the intake window with one batch left in
flight; taking recorded requests in once more". Before the cutoff,
with more than one flight, or with nothing pending, behaviour is
unchanged, and the next run still reloads config and tools. The
window length is `RunOptions::intake_for`, set by
`serve --intake-for-secs` and defaulting to `INTAKE_FOR`, as the
Design's testability note records. Covered by the maintained test
"late resubmission while one long batch drains"
(cli_intake_window: ejection, solo rebuild, window expiry, late
round beside the draining batch, a further submission left for
the next run), written failing first. Gate: full gate on
092d7072, 239 steps, rust.merge-queue.test clean at 582 passed /
0 failed (552s); the one failure was untouched
python.wrkslots.shard-3 (a census timing case), green in its
solo exclusive rerun (150 passed, rerun_passed); the exclusive
cross differentials were stopped under the merge-queue-only
waiver. Main moved under the gated base (the exact/sync/
plan-intake stack), so the replayed combined tree was verified
(fmt, release clippy -D warnings, client-names, whole
merge-queue crate in release, all clean) and landed with
--allow-overlap; the landed tree is byte-identical to that
verified tree.
