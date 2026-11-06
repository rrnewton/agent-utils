---
title: 'gate-pytest-basetemp-leak: gates leak pytest basetemp directories in the shared temp dir'
status: closed
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-06T14:30:00.000000000+00:00
updated_at: 2026-11-06T14:30:00.000000000+00:00
---

# Description

Gates leave pytest basetemps under /tmp/pytest-of-USER. pytest's keep-three retention does not prune them because each pytest process numbers its own. One host had 85 dirs (29 GiB), some with read-only subdirectories that plain removal cannot delete.

# Design

Point pytest's --basetemp into the gate's own scratch directory (one per gate run), and remove it at gate end, making read-only test dirs writable first. Never leave basetemps in the shared temp dir.

# Acceptance Criteria

- After a gate finishes, pass or fail, no basetemp it created remains.
- Read-only directories created by tests are removed too.
- Concurrent gates do not share or delete each other's basetemps.

# Notes

Filed 2026-11-06 9:30 AM ET at the lead coordinator's request, from one host's gate ledger and disk survey.

## Resolution

[opus 5.5] cl-start, 2026-11-06.
- Cause: sharded pytest steps (`run_pytest_shard.py`) already used a private `--basetemp` and removed it. Every other pytest process in a gate (component suites, nested and cross-harness sessions, pytester) numbered its own `pytest-N` under the shared `/tmp/pytest-of-USER`, so the keep-three retention never caught up. This host had 25 entries, 8.6 GiB, including `garbage-*` directories pytest itself could not delete.
- Fix: `scripts/validate.py` gives each run a private root, `$TMPDIR/validate-pytest-PID-XXXX`, and sets `PYTEST_DEBUG_TEMPROOT` to it for the whole graph, including the gate-exclusive phase.
  - Inside a run, pytest's own numbered, locked directories keep concurrent processes apart. No two runs share a root, so none numbers, prunes or deletes another's.
  - When the run ends, passed or failed, the root is removed. Owner access is restored first, so read-only test directories go too. A SIGTERM ends the run through the same cleanup.
  - A root a harder kill left behind is swept by a later run once it is this user's, untouched for 24 hours, and its PID is gone.
- Tests:
  - Two runs get distinct roots, and both are gone afterwards, including a mode-0 directory.
  - The graph's environment carries the root.
  - The sweep removes a stale dead root and keeps live and fresh ones.
- Not done: the existing backlog under `/tmp/pytest-of-USER` from earlier gates is left for an operator to remove while no gate runs.
