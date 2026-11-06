---
title: 'gate-flake-stop-busy-runner: python.agentctl.shard-1 test_stop_busy_runner fails under load on signal timing'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T14:30:00.000000000+00:00
updated_at: 2026-11-06T14:30:00.000000000+00:00
---

# Description

python.agentctl.shard-1 failed in 5 first-run gates in about 24 h and passed each time it was rerun alone. The failing test is test_stop_busy_runner (the signal-codex variant), which times a SIGTERM against a busy fake runner.

# Design

Make the test wait on an observable event (the runner reporting busy, the signal delivered) instead of a fixed time budget, or scale its timeouts by measured load, without weakening what it asserts.

# Acceptance Criteria

- 20 consecutive runs pass under the bounded contention used for #905 (stress in a CPUQuota scope).
- The test still fails if stop does not deliver the signal.

# Notes

Filed 2026-11-06 9:30 AM ET at the lead coordinator's request, from one host's gate ledger and disk survey.
