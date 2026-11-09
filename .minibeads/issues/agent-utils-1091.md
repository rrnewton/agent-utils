---
title: 'audit-lock-free-row-flake: an unchanged slot is marked UNKNOWN when another slot changes during the audit, under gate load'
status: closed
priority: 3
issue_type: bug
created_at: 2026-11-08T20:50:00.000000000+00:00
updated_at: 2026-11-09T00:37:58.000000000+00:00
closed_at: 2026-11-09T00:37:58.000000000+00:00
---

# Description

`py/wrkslots/tests/test_audit_lock_free.py::test_audit_own_record_change_marks_only_that_row_unknown` failed in a full gate on devbig030: cl-start's #1064 test-temp-and-mount-leaks re-gate of 98438f6a, step `python.wrkslots.shard-4`, which finished 2026-11-08 15:33 ET. It passed in the solo rerun of the same tree and in the previous gate. #1064 does not touch the audit code.

The test changes slot-a's record during the census and expects only slot-a's row to become UNKNOWN. The two slot-a assertions held. The third, at test line 220, failed: slot-b's row was also UNKNOWN (`assert 'UNKNOWN' != 'UNKNOWN'`). The visible part of that row was `{'agent': 'agent-slot-b', 'cache_bytes': 0, 'cache_error': None, 'cache_status': 'complete', ...}`; pytest truncated its reasons.

So under load, slot-b was judged changed or unverifiable even though only slot-a's record changed. Possible causes are a load-sensitive timing or budget check that reaches every row, or the registry rewrite in `_write_active` being seen as touching slot-b too.

# Design

- Reproduce under load and capture slot-b's `reasons`, which name why it became UNKNOWN.
- Make an unchanged slot's verdict independent of timing. If a load-dependent bound can legitimately make every row UNKNOWN, have the test separate that case from the own-record case.

# Acceptance Criteria

- slot-b's UNKNOWN reason under load is named.
- The test passes repeatedly under gate-like load.

# Notes

Filed by cl-start at the devbig030 coordinator's request. Related: #1084 audit-census-reserve-flake, another wrkslots audit test that fails only under gate load.

## Resolution

slot-b's UNKNOWN reason under load: "process/path census failed: ..." -- the census refusal marks every census-eligible row UNKNOWN, slot-b included, independent of which record changed.

Cause, shared with #1091 audit-lock-free-row-flake / #1084 audit-census-reserve-flake: the Python audit's host-wide process/path census (`_capture_process_path_census`: three attempts, an attempt discarded on any /proc churn, one 60 s budget) refuses on a loaded host (gate load averages 100-235), and every row that consulted it becomes UNKNOWN with "process/path census failed: ...". The Rust audit lists /proc once and ignores vanished entries, so it answers where Python refuses. Confirmed without load: a plugin making the census refuse reproduces the gate failures exactly (all five lock-free tests seen in gate logs, and the differential diffs: Python slot-a UNKNOWN, Rust actionable).

Fixed (cl-wrkid, 2026-11-08) for every audit test in test_audit_state_rust.py and test_audit_lock_free.py: they now use the `pinned_process_evidence` fixture (py/wrkslots/tests/conftest.py), which answers "no process uses these paths" after verifying that with one churn-tolerant /proc pass, and fails the test loudly, naming the process, if it is false. Unpinned (marker `real_process_evidence`): test_audit_deletable_matches_python (real census, both editions), test_audit_measures_outside_the_state_lock (real census), and the two census-behaviour tests that supply their own census. The product fix and the parity decision are #1102 wrkslots-census-churn-tolerant, with strict-xfail forced-churn tests (both editions miss a PID that appears after the listing). Not covered here, tracked under #1102: the remove/reclaim/recover tests that flake on the sibling 30 s /proc-scan bound.
