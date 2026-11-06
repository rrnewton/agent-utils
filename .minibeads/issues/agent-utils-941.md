---
title: 'gate-pytest-basetemp-leak: gates leak pytest basetemp directories in the shared temp dir'
status: open
priority: 2
issue_type: bug
assignee: mu-validate
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
