---
title: 'probe-no-counted-tests: a base probe with no counted tests must not read as a red base'
status: open
priority: 0
issue_type: bug
created_at: 2026-11-11T06:00:00.000000000+00:00
updated_at: 2026-11-11T06:00:00.000000000+00:00
---

# Description

A base-only probe whose selection runs no counted tests (here check.lint_checks alone) exits 4. The validator's receipt reports row_verdict=NOT-PASSED because executed_tests is null ("a ledger row with executed_tests=null cannot qualify a receipt"), even though every node returned 0 and FINAL_VALIDATE_STATUS is PASSED.

The queue reads that exit 4, with no failed test, as an unattributed red on the base, and it stopped a consumer run with "the base <sha> itself fails red ... fix the base first".

Risk: if the candidate fails the same lint-only node again, the queue probes the base again and loops, at about 25 min per cycle.

# Design

In `rs/merge-queue`:

- For a PROBE, a NOT-PASSED receipt whose nodes all exited 0 and whose final status is PASSED counts as pass (or at least as "inconclusive: no counted tests"), which must never eject or stop a run.
- Loop guard: the same probe selection on the same base with an unchanged inconclusive result is not re-run.

# Acceptance Criteria

- A base probe with no counted tests, all node exits 0 and final status PASSED, does not stop the run as a red base and ejects nothing.
- The same inconclusive probe on the same base is not re-run in a loop.
- `cargo test -p merge-queue` (all targets) passes, including the new cases.

# Notes

Reported 2026-11-11. The consumer side (the validator's receipt rule) may change too, but the queue must be robust either way.
