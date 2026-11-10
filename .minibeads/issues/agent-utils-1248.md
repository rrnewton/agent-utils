---
title: 'fold-latency-behaviour-verdict: the fold check judges behaviour, and its millisecond budgets are an opt-in benchmark'
status: open
priority: 2
issue_type: task
assignee: mu-foldlat
created_at: 2026-11-10T21:05:00+00:00
updated_at: 2026-11-10T21:05:00+00:00
---

# Description

[muse watermelon] `vibe-talk.check.fold-latency-browser` judged handler milliseconds against a
28 ms budget, so a hot host could fail it while the folds behaved, and tuning that budget under
load kept chasing contention. Per the owner's rule that real-time-sensitive tests leave gating
until they are robust, the check's gating verdict becomes the behaviour alone: eight Shift+O
presses draw eight folds as one line, eight `e` presses draw eight Done marks, and hiding a row
restyles only what it touches. The millisecond budgets move to an opt-in `--benchmark` run
outside the gate, and the fresh majority re-walk is removed: one walk, judged once.

# Design

- The gating verdict is behaviour only: every press heard, every watched row drawn folded or
  Done, the folds one line, the restyle count within its bound. Every press's handler time is
  still printed with the host's load, but no time decides the verdict.
- `--benchmark` enforces the former budgets (work at the median, twice that for all but the
  largest quarter of presses, wall at 8x) as a speed measurement nobody gates on.
- Waits stay on drawn-state events and settled geometry, so the check is deterministic on a
  hot machine.

# Acceptance Criteria

- 20 of 20 runs inside a CPU-quota scope saturated by burners, plus passes under the host's
  natural load and solo.
- A variant page whose fold behaviour is broken still fails; the busy-work variant fails only
  under `--benchmark`.
- `keyboard-browser` and `keyboard-hides-bar-browser` stay enabled as they are; this issue
  supersedes the keyboard CPU-timing deflake.

# Notes

Owner redirect 2026-11-10 (real-time-sensitive tests out of gating until robust). The keyboard
and keyboard-hides-bar checks were proved the same way (20 of 20 in the saturated scope, their
behaviour mutations red) and need no change.
