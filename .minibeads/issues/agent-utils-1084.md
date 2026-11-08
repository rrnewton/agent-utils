---
title: 'audit-census-reserve-flake: the finalize-reserve audit differential disagrees under gate load'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-08T19:10:00.000000000+00:00
updated_at: 2026-11-08T19:10:00.000000000+00:00
---

# Description

`py/wrkslots/tests/test_audit_state_rust.py::test_audit_census_finalize_reserve_matches_python` failed in a full gate and passes when run alone. The gate was cl-start's first #1064 test-temp-and-mount-leaks gate on devbig030, step `python.wrkslots.shard-2`, which finished 2026-11-08 14:04 ET. That change does not touch the audit code or this test.

What failed: the second, `work_limit=1` pass. Both editions exited 0, but their JSON differed in the slot row (`assert _scrub(json.loads(rust_second.stdout)) == _scrub(python_second)`, test line 653). pytest truncated the diff. The visible part of the row shows `cache_status: error` and this `cache_error` text: "fresh cache finalization requires 21 work units for this subject, exceeding the fixed allowance of 1; no cache total can be published within this allowance". So both editions computed 21 work units against an allowance of 1, and the difference lies in a field the truncated diff did not show.

Run alone on the same tree (`python3 -m pytest wrkslots/tests/test_audit_state_rust.py::test_audit_census_finalize_reserve_matches_python`), it passes in 12 s. That points to something load- or time-sensitive in one edition's row: a timestamp, an age, or a budget that counts elapsed time.

# Design

- Reproduce under load, for example with the shard running beside other gate steps or with CPU contention. Capture the full row diff with `-vv`, or print both rows when they differ.
- Find which field differs and why it depends on load, and make both editions agree, or scrub the field in the test if it is legitimately time-dependent.

# Acceptance Criteria

- The differing field is named, with the reason it varies under load.
- The test passes repeatedly under gate-like load, for example 20 runs beside a CPU-heavy neighbour.

# Notes

Filed by cl-start at the devbig030 coordinator's request; not fixed in #1064.
