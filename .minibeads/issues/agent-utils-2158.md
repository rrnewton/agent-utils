---
title: 'vibe-talk-thread-pin-picker-browser-date-timebomb: the thread pin picker browser check hard-codes fixture ages that grow with the date'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- flaky-test
created_at: 2026-11-23T07:00:00-05:00
updated_at: 2026-11-27T17:37:18-05:00
closed_at: 2026-11-27T17:37:18-05:00
---

# Description

vibe-talk.check.thread-pin-picker-browser now fails deterministically on every tree: it asserts fixture message ages as fixed strings (for example "45w" and "14w"), but the seeded dates are absolute, so the rendered ages grew to "46w" and "15w" as time passed. It is red in every vibe-talk gate today, regardless of the change under test.

# Design

Make the check date-independent: freeze the page clock for the test (the existing time seam), or seed fixture dates relative to the test's now, or assert the ages computed from the seeded dates. Audit other browser checks and page tests for the same pattern (hard-coded relative ages over absolute fixture dates).

# Acceptance Criteria

- The check passes today and with the clock moved a year forward; it still fails if the chronological order shown is wrong.

# Notes

Found by a lane's gate on 11-23; filed by the coordinator.

## Resolution

Fixed by 5d357dc9 (vibe-talk: compute the pin picker's expected ages from its seeds), landed 2026-11-23 07:57 ET. The expected ages are computed from the fixture's seeded updated_at stamps with the page's own brief-age rule.

Correction for the 2026-11-27 frequent-red-steps audit, which read "17 untouched heads since #2158" as contradicting this issue. It does not:
- All 18 thread-pin-picker reds from 2026-11-23 13:14Z to 11-24 01:09Z were on heads that did NOT contain 5d357dc9. Their base had it, but the gate tests the head's own tree, and those lanes had branched before the fix. They were this issue's real date bomb.
- Since then, only two reds were on heads that contain the fix:
  - 7c346872 (11-25): a tree with 45 failed steps.
  - c3f49b05 (11-27): a different, real race, the slug read before the thread summaries draw it. Its rerun passed. It is tracked in #2416.
