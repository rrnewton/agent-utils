---
title: 'recover-late-census-hot-host: real-holder recover census test fails on a hot host'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-20T00:00:00+00:00
updated_at: 2026-11-20T00:00:00+00:00
---

# Description

[watermelon-full-preview] `wrkslots/tests/test_recover_late_census_rollback.py::test_real_holder_late_census_refusal_in_recover_unfences`
(from `#1923`) failed in the `python.wrkslots.shard-2` gate step on
2026-11-20 (gate on `58414625`, formatting-only tree) with host load
average 75-121:

```
E       assert 'live process 364845 uses slot' in "REFUSED: same-UID process census did not close: each of 8 process inventories found generations that forked or exited..."
wrkslots/tests/test_recover_late_census_rollback.py:248: AssertionError
1 failed, 159 passed in the shard
```

On a hot host the real same-UID census legitimately does not close
within its 8 inventories because unrelated generations keep forking and
exiting, so recover refuses with the census-not-closed reason instead of
the live-holder reason the test asserts.

# Design

- Owner rule 346: a gating test must be robust on a host above 75% CPU.
  Make the refusal event-based (a holder/census driver whose generations
  are born and die at census inventory points, in the style of the
  scripted-host census tests but against real processes), or disable the
  test with this tracking issue until that is done. No wall-clock budget
  is an acceptable fix.

# Acceptance Criteria

- The test passes repeatedly (at least 5 consecutive runs) on a host
  above 75% CPU, or is disabled with this issue named as the tracker.
- The deterministic census-refusal coverage (scripted inventories) is
  unchanged.

# Notes

Filed by mu-flakes4 at coordb's direction (number reserved by coordb).
Solo-rerun evidence is recorded in the gate ledger on `58414625`.
