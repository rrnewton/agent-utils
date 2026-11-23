---
title: 'wrkslots-scoped-status-heartbeat-tick-flake: a status comparison test fails when the heartbeat age ticks between two calls'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- flaky-test
created_at: 2026-11-22T22:35:00-05:00
updated_at: 2026-11-22T22:35:00-05:00
---

# Description

py/wrkslots/tests/test_event_index.py::test_scoped_status_ignores_a_tampered_snapshot_row compares a scoped status with a full status taken a moment later. The output includes heartbeat_age_seconds, which can tick from 0 to 1 between the two calls, so the comparison fails. Reproduced alone at about 1 in 5 runs, more often on a loaded host because slower calls widen the window. This is a wall-clock dependency in a gating test, against the owner rule that gating checks never depend on real time.

# Design

Compare the two statuses with time-derived fields (heartbeat_age_seconds and similar ages) removed or normalised, or freeze the clock for both calls through the existing time seam. Audit the same file for other comparisons of age fields.

# Acceptance Criteria

- The test passes 50 times in a row alone and under load, and still fails if the tampered row leaks into the scoped status.

# Notes

Found by a lane while triaging a gate; filed by the coordinator.
