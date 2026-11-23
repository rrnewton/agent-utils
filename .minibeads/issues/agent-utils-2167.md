---
title: 'lifecycle-fuzz-wall-clock-limits: the gating fuzz test fails on communicate() timeouts on hot hosts'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-22T19:00:00.000000000-05:00
updated_at: 2026-11-22T19:00:00.000000000-05:00
---

# Description

`test_lifecycle_fuzz_fast_seeds` (py/wrkslots/tests/test_lifecycle_fuzz.py) has failed in three consecutive land1 gates (77d54b2e and predecessors) as `subprocess.TimeoutExpired` inside a wrkslots CLI call, while passing solo 5/5 on the rebased tree (600-887s per invocation) and repeatedly standalone. The harness already waits condition-based (decode polling after every step); the failures are fixed wall-clock ceilings — e2e's interactive 120s subprocess bound, per-seed subprocess bounds, and gate freeze windows during which wall clocks kept ticking — being used as pass/fail criteria on a saturated host, against the owner's no-timing-in-gates rule. One census-heavy wrkslots call legitimately takes minutes when the host runs several gates at once.

# Design

Keep every wait event- or condition-based: decode polling after each step (already the harness shape), no `communicate()` wall-clock limit as a pass/fail criterion. Process ceilings remain only as generous anti-hang bounds (600s per CLI call, shared `_CLI_TIMEOUT` in lifecycle_fuzz.py, landed ahead of this issue) and are not evidence of failure by themselves; a seed that exceeds them reports its trace instead of dying inside a nested communicate. Until that is proven across gates, the fast-seeds test is quarantined with the repo's `load_quarantine` marker (#2167 named in the marker registry), so it stops failing unrelated gates while staying runnable on demand (`WRKSLOTS_INCLUDE_QUARANTINED=1` or `-m load_quarantine`). The SIGTERM and failing-run teardown tests keep gating (they are fast and mount-scoped).

# Acceptance Criteria

Fast-seeds runs green in a full gate on a loaded host (several gates concurrent) without any wall-clock limit acting as the failure, and solo passes continue (currently 5/5 at 600-887s). The quarantine marker is removed in the same change that demonstrates this; until then the test is named in the gate terminal summary as quarantined, not failed. No sleep-as-sync anywhere: all waits stay decode-polling or process-completion conditions.

# Notes

[watermelon-full-preview] Filed from #2043 land1 gates at the coordinator's instruction; failure evidence in the gate logs for 77d54b2e (shards 8/10/11 across runs, always TimeoutExpired in a CLI communicate, never an oracle assertion). The 600s ceiling and gated-bound raise already landed as the hot-host timeout fix; this issue tracks removing the remaining wall-clock pass/fail surface and un-quarantining.
