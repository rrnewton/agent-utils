---
title: 'vibe-talk-thread-pin-picker-browser-date-timebomb: the thread pin picker browser check hard-codes fixture ages that grow with the date'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- flaky-test
created_at: 2026-11-23T07:00:00-05:00
updated_at: 2026-11-23T07:00:00-05:00
---

# Description

vibe-talk.check.thread-pin-picker-browser now fails deterministically on every tree: it asserts fixture message ages as fixed strings (for example "45w" and "14w"), but the seeded dates are absolute, so the rendered ages grew to "46w" and "15w" as time passed. It is red in every vibe-talk gate today, regardless of the change under test.

# Design

Make the check date-independent: freeze the page clock for the test (the existing time seam), or seed fixture dates relative to the test's now, or assert the ages computed from the seeded dates. Audit other browser checks and page tests for the same pattern (hard-coded relative ages over absolute fixture dates).

# Acceptance Criteria

- The check passes today and with the clock moved a year forward; it still fails if the chronological order shown is wrong.

# Notes

Found by a lane's gate on 11-23; filed by the coordinator.
