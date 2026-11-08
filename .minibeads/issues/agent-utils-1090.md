---
title: 'agentctl-launch-proc-scan-flake: the background-descendant test crashes reading /proc of a process that is exiting'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-08T20:50:00.000000000+00:00
updated_at: 2026-11-08T20:50:00.000000000+00:00
---

# Description

`py/tests/test_agentctl_client_launch.py::test_real_supported_shell_with_background_descendant_is_not_idle` failed in a full gate on devbig030: cl-start's #1064 test-temp-and-mount-leaks re-gate of 98438f6a, step `python.agentctl.shard-1`, which finished 2026-11-08 15:33 ET. It passed in the solo rerun of the same tree and in the previous gate.

The traceback ends in the test's own `child_exists()` helper (test line 1561, called from the assertion at line 1571). The helper walks `/proc` and reads every `<pid>/stat`, but it catches only `FileNotFoundError` and `PermissionError`. Reading the stat file of a process that exits between `iterdir()` and `read_bytes()` raised `ProcessLookupError: [Errno 3] No such process` from pathlib, which escaped and failed the test. Under gate load, other processes exit all the time.

This is the Python twin of #499 agentctl-fixture-poison-flake and #495 revive-stat-parse-flake. In the Rust edition of the same test, the same race appears as "cannot parse process stat for pid N".

# Design

- In `child_exists()`, treat `ProcessLookupError` (or any `OSError` from a vanished `/proc` entry) like `FileNotFoundError`: skip that entry.
- Check the other Python `/proc` walkers in agentctl's tests and code for the same narrow except clause.

# Acceptance Criteria

- A `/proc` entry whose stat read raises ESRCH is skipped, not raised. A test fakes that read.
- The test passes repeatedly under gate load.

# Notes

Filed by cl-start at the devbig030 coordinator's request.
