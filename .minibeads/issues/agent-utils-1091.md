---
title: 'audit-lock-free-row-flake: an unchanged slot is marked UNKNOWN when another slot changes during the audit, under gate load'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-08T20:50:00.000000000+00:00
updated_at: 2026-11-08T20:50:00.000000000+00:00
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
