---
title: 'gate-browser-steps-under-load: vibe-talk browser checks still fail under shared-gate load and cost every lane a solo rerun'
status: open
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-07T02:30:00.000000000+00:00
updated_at: 2026-11-07T02:30:00.000000000+00:00
---

# Description

After #905 browser-latency-load-sensitivity (closed 2026-11-06, f605b64e: the fold check's per-press ceiling became the second-largest press at 2x the budget), `vibe-talk.check.fold-latency-browser` still failed in full shared gates on 4 different heads on 2026-11-06, for example desk Shift+O handlers at 22-29 ms against a 21 ms budget. `vibe-talk.check.read-aloud-blocks-browser`, `vibe-talk.check.pairing-link-browser` and `vibe-talk.check.read-aloud-views-browser` each failed once in the same way. Every failure passed in a solo `gate.py --exclusive` rerun on the same commit.

Each failure costs the lane a solo rerun that waits behind the shared gates, about 45 minutes of queue time, and it holds that lane's landing.

# Design

Pick one with measurement and record the choice here:

- Make the assertions load-aware: measure main-thread or CPU time (performance marks, long-task timing) against the budget instead of wall time, or compare against a baseline calibrated on the same host at the start of the step. A real main-thread regression must still fail.
- Or move these browser steps into the gate's exclusive phase, where they run alone, and measure what that adds to a full gate's wall time.
- No sleeps, retries-until-green or silent skips. A skip needs a named, measured reason, per #905's rule.

# Acceptance Criteria

- Under the same bounded contention #905 used (or a full shared gate running beside another), 20 consecutive runs of each affected step pass on an unchanged commit.
- An injected main-thread regression (e.g. a busy loop in the fold handler) still fails the fold check under that load.
- The issue records the choice, the measurements, and the effect on full-gate wall time.

# Notes

Requested 2026-11-06 by the lead coordinator. Related: #905 (earlier fix), #725 (CPU-budget rule).
