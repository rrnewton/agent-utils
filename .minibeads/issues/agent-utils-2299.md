---
title: 'finish-archive-remove-budget: the ignored-only archive test fails on untouched main under load'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- tests
created_at: 2026-11-26T11:04:50-05:00
updated_at: 2026-11-26T11:04:50-05:00
---

# Description

[watermelon-full-preview] `wrkslots/tests/test_finish_provisioned.py::test_ignored_only_archive_finishes_with_verified_tar` fails intermittently on untouched main under host load: finish succeeds and the tar verifies, then the remove step refuses because the remove-agent-batch item used its 60-second budget for work before deletion at the /proc scan after the path fence (budget phase: salvage). Measured 2026-11-26: solo on clean main 2 passed, 1 failed with exactly that refusal (the failing run at 1-minute load 249.77, 182s wall for a test whose passing runs take 74-149s); on a contemporary feature tree 5/5 passed. The test's remove runs against an internal wall-clock budget while the host is saturated, which is the shape the owner rule (load-invariant testing: condition-based waits, no wall-clock budgets) forbids depending on.

# Design

Two acceptable outcomes, in this order:
(1) Make the test condition-based: the remove under test should not depend on the 60-second salvage budget expiring under load — for example by driving the remove with `--no-lock-budget` (the refusal's own remedy, which has no such budget) if the budget is not what the test means to exercise, or by waiting on the salvage condition itself rather than the clock.
(2) If the budget behaviour IS what the test means to exercise, assert the refusal-or-completion disjunction explicitly (budget refusal is a correct product outcome under load) instead of asserting success only.
If neither lands promptly, disable the test under this issue (marked with the issue reference and the measured evidence) rather than letting it flake gates.

# Acceptance Criteria

- The test passes 10 consecutive solo runs on a loaded host (1-minute load above 150) on main, or is disabled with this issue named and the measurements recorded.
- No product behaviour changes: finish, archive verification, and remove budget semantics are untouched unless the investigation proves the budget accounting itself is wrong (then that becomes its own change with tests).

# Notes

Filed from the #2000 gate triage (mu-restoreboot, 2026-11-26): the test failed in python.wrkslots.shard-4/10 rows on a lock-order tree, passed 5/5 solo on that tree, and failed 1 of 3 solo on untouched main at load1 249.77 with the identical salvage-budget refusal. Related: #2000 wrkslots-lock-order (the gate that surfaced it), #2065 (census-family quarantine — same host-load class, different mechanism).
