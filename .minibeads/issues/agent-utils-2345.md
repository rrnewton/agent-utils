---
title: 'slot-images-errno-substring: the first-error usage test fails whenever the pytest process ID contains 107'
status: open
priority: 2
issue_type: bug
assignee: cl-sysdexpand
labels:
- wrkslots
- tests
created_at: 2026-11-26T21:50:00-05:00
updated_at: 2026-11-26T21:50:00-05:00
---

# Description

`test_usage_keeps_the_first_error_when_stat_and_statvfs_both_fail` in `py/wrkslots/tests/test_slot_images.py` checks that `slotimage.usage` keeps the first error (the slot image stat failing with `ENOENT`) instead of overwriting it with the later `statvfs` failure (`ENOTCONN`, errno 107). It asserts:

```python
assert "Errno 2" in str(slot_entry["error"])
assert "107" not in str(slot_entry["error"])
```

The error text is `[Errno 2] No such file or directory: '<path>'`, and `<path>` is under the pytest temporary directory, whose name carries the pytest process ID (`pytest-<pid>-<n>`). Whenever that process ID contains the digits `107`, the second assertion fails although the code behaves correctly. A full gate on an unrelated tree went red on `python.wrkslots.shard-5` this way: the pytest process ID was 1210783.

The failure depends only on the process ID, not on load or timing, so it recurs at random on any tree.

# Design

Test-only change. Assert the absence of the second error by its own words, not by a bare number that a path can contain:

- `"Errno 107" not in` the error text, and
- `os.strerror(errno.ENOTCONN) not in` the error text.

Keep the `"Errno 2"` assertion. No product code changes.

# Acceptance Criteria

- The test no longer fails when the temporary path contains `107`. Proved by a run with the temporary directory forced to a base path containing `107` (`--basetemp`), which fails before the fix and passes after it.
- The test still fails if `usage` reports the `statvfs` error instead of the stat error. Proved by a local mutation that makes the later error win.
- No other test asserts that a bare number is absent from text that embeds a temporary path (checked by a repository search).

# Notes

Found while triaging the reds of an unrelated gate (#2316). Positive substring checks on bare numbers elsewhere (`"17" in output`, `"3072" in joined`) can only pass by accident, never fail by accident, so they are out of scope here.
