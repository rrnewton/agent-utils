---
title: 'wrkslots-scoped-status-heartbeat-age-flake: a status equivalence test compares two reads across a second boundary'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-24T12:50:00-05:00
updated_at: 2026-11-24T12:50:00-05:00
---

# Description

py/wrkslots/tests/test_event_index.py::test_scoped_status_ignores_a_tampered_snapshot_row (and its sibling equivalence cases) compares a scoped `status --slot` row against the full unscoped `status` row from two separate calls. When the calls straddle a one-second boundary, the rows differ in exactly one field: heartbeat_age_seconds is 0 in the first read and 1 in the second. The test then fails with a whole-row mismatch that looks like a scoped-status defect. Reproduced on clean github/main (1 failure in 3 solo runs, with further runs passing) and on an unrelated change's tree (2 of 3, then 1 of 5 in verbose runs); the verbose diff shows heartbeat_age_seconds as the sole differing field. This violates the hot-machine rule (346): a gating test must not depend on wall-clock boundaries.

# Design

Make the comparison time-free: exclude heartbeat_age_seconds from the row comparison (or normalise it in both rows before comparing), or freeze the clock the status rows are computed against, in the affected equivalence tests. The rest of each row must still compare exactly, so a corrupted scoped row still fails.

# Acceptance Criteria

- The affected equivalence tests pass repeatedly on a loaded host with no dependence on second boundaries.
- A deliberately corrupted scoped row still fails each test.
- No other assertion in the tests is weakened.

# Notes

Diagnosed while triaging an unrelated change's gate (the failing shard was first blamed on that change; the sole-differing-field evidence cleared it). The scoped-status code itself is correct: both reads are right, one second apart.
