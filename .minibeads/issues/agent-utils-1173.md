---
title: "census-vanished-process: a process that exits mid-census makes slot use indeterminate instead of gone"
status: open
priority: 1
issue_type: bug
created_at: 2026-11-10T00:20:00.000000000+00:00
updated_at: 2026-11-10T00:20:00.000000000+00:00
---

# Description

The process/path census refuses as indeterminate when a process exits while it is being read. Seen twice in one
full gate on 2026-11-09 (rust.wrkslots.test and repository.check.wrkslots-debug-test, both in
`tests::real_python_create_and_import_recoveries_close_their_marker_only_with_a_row`, wrkslotsd/src/tests.rs:6821):

    REFUSED: process use is indeterminate because /proc/<pid>/maps cannot be read: [Errno 3] No such process

A process that vanished between listing /proc and reading one of its entries no longer uses anything, so the
census should treat it as gone. #1102 wrkslots-census-churn-tolerant made the census tolerate churn, but this
path still refuses. A similar /proc race hit a cross-language differential gate step the same night, possibly the
same family.

# Design

- During the census, treat ESRCH and ENOENT on any `/proc/<pid>/*` read as "this process is gone": skip it and
  record it in the census diagnostics. Cover every per-process read: maps, fd (the directory and each link), cwd,
  root, exe and environ, and any other per-pid file the census opens.
- Keep "indeterminate" for EACCES, EPERM and every other error, so an unreadable live process still refuses.
- Python (`cli.py` around the "process use is indeterminate" sites, ~21233-21266) and the Rust census readers
  agree: same classification and same diagnostics.

# Acceptance Criteria

- Tests with an injected ESRCH and an injected ENOENT on each per-process read: the census completes and the
  process is not counted as a user.
- Tests with an injected EACCES: still indeterminate (refusal unchanged).
- A churn test (processes spawning and exiting during the census) passes repeatedly in both editions.
- The failing differential test above passes under load (5 consecutive runs, as a touched-test bar).

# Notes

Requested at P1 by the lead coordinator (2026-11-09).
