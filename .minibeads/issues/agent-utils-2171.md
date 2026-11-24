---
title: 'gate-memory-floor-test-wallclock: the memory-floor wait test races subprocess startup against a 1s sleep'
status: open
priority: 2
issue_type: bug
labels:
- gate
- flaky-test
created_at: 2026-11-23T19:30:00-05:00
updated_at: 2026-11-23T19:30:00-05:00
---

# Description

test_memory_floor_waits_then_admits_and_is_recorded sleeps 1 second, then raises the fake meminfo, assuming the subprocess gate has already printed its first below-floor poll. Under gate load the subprocess startup outlasts the sleep, so the gate first polls after the raise, admits at once, and the test fails on the missing "waiting" line. It failed that way in the known-red removal gate on 2026-11-23 while the same test passed its solo rerun.

# Design

Wait on a condition, not a duration: observe the gate's own "memory-floor: waiting" output (or a readiness record it writes) before raising the fake meminfo, with only a generous wall-clock backstop. No assertion depends on how fast the subprocess starts, per the owner's hot-machine rule.

# Acceptance Criteria

- The test passes under load and with a slowed subprocess start; it still fails if the gate does not wait below the floor.

# Notes

Filed by the coordinator (#2171 gate-memory-floor-test-wallclock); the owning lane refines this file as it works.
