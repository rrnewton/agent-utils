---
title: 'retire-prepare-foreign-events-condition: the retire lock-free prepare test waits for its prepare to reach the lock-free phase instead of failing when contention defers it'
status: open
priority: 2
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- tests
- flaky-test
created_at: 2026-11-26T22:40:00-05:00
updated_at: 2026-11-26T22:40:00-05:00
---

# Description

`py/wrkslots/tests/test_retire_lock_scope.py::test_retire_pending_survives_foreign_events_during_lock_free_prepare` injects another slot's create and heartbeat at the moment `_prepare_retirement_removal` returns a prepared removal, then asserts the injection fired (`injected == ["foreign"]`) and the slot was removed. When the prepare does not return a prepared removal (under lock contention its attempt-scope lock wait defers it), the injection never fires and the test fails with `assert [] == ["foreign"]`, although nothing it means to test went wrong. Seen during the `#2000` combined-tree checks in a 10-minute focused bundle at load1 about 150; the same tree passes alone (92.9 s), and a traced solo run shows the prepare returning a prepared removal with and without the lock-order check.

# Design

Test-only. Find what defers the prepare in the bundle (the attempt-scope lock wait and its deadline, or another holder of the mutation locks) and make the test wait on the condition it needs: the prepare reaching its lock-free phase with a prepared removal, with the suite's hang backstop, rather than one shot that a deferred attempt turns into a failure. The assertions about the outcome (the prepared removal survives the foreign events, `slot01` is removed, the foreign slot stays) do not change. If the deferral comes from a fixed wall-clock lock wait in the product, say so here and route that part to `#2219` (CPU-time budgets) instead of changing it.

# Acceptance Criteria

- The test passes 10 consecutive runs inside the same focused bundle on a host at load1 of at least 150, and on main alone.
- A deferred prepare no longer fails the test by itself; a prepared removal that the foreign events do defer still fails it.
- No product change; no wall-clock budget added (owner rule 346).

# Notes

Reported by `mu-restoreboot` (2026-11-26) from the `#2000` wrkslots-lock-order combined-tree checks. Same file as `#1947` retire-lock-scope-flaky-budget (closed: the 60 s item budget), different mechanism. Queued in `cl-flaketriage` after its current items (coordinator, 2026-11-26).
