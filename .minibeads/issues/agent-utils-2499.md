---
title: 'host-census-wallclock-bound: the host-mode read-only census refuses after a 120-second operation-wide wall-clock bound, so a sealed remove fails under host load'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- census
- hot-host
created_at: 2026-11-28T15:30:00-05:00
updated_at: 2026-11-28T15:30:00-05:00
---

# Description

`py/wrkslots/tests/test_slice_fallbacks.py::test_sealed_remove_completes_through_the_teardown_closure` (an `ordinary_environment` case, run in the host shards) failed in gate 1795894776097312074-bd122e61 (`python.wrkslots.host-0`, 2026-11-28) with `assert 3 == 0`. The remove refused with:
- "REFUSED: privileged read-only census stderr exceeded 8387 bytes"
- "REFUSED: read-only census exceeded its operation-wide time bound (bound 120.0 s, elapsed 120.0 s)"

The census over the host's process table runs under a fixed operation-wide wall-clock bound, 120 s. On a busy host the census takes longer, and the remove refuses for a reason that names no fault in the slot. The stderr bound was exceeded too, from diagnostics about unrelated host processes.

This is the owner's hot-machine rule again (rule 346): a normal operation refuses on wall-clock time under load. It relates to #2497 (census bounds) and to #2459 (the remove item budget).

# Design

- Replace the wall-clock bound with a load-invariant one (CPU time), or remove it.
- Keep a generous hang backstop sized against the clients that wait on the lock, not against load. The same direction as #2459.
- The stderr bound counts only diagnostics about processes that can matter to the census, not unrelated host churn.
- Both editions get the same rule, with a differential test.
- No new persisted field.

# Acceptance Criteria

- A sealed remove on a host at load1 >= 237 completes, or refuses for a named cause that is not wall-clock.
- The case passes in its host shard in a gate.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the #2493 gate. Unassigned.

Related:
- #2497: census bounds.
- #2459: the per-item remove wall-clock budget.
