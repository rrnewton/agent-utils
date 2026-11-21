---
title: 'gate-step-logs-kept: keep each gate step''s log and record its path in the ledger'
status: open
priority: 2
issue_type: bug
labels:
- gate
created_at: 2026-11-21T15:00:00-05:00
updated_at: 2026-11-21T15:00:00-05:00
---

# Description

A gate's per-step logs are not kept anywhere durable: the ledger's log_path is always empty, and neither gate.py nor validate.py sets the dagrun log directory, so after a red gate only the failed step names and whatever the lane's own redirected output captured survive. Diagnosing load-related failures across gates needs the step logs.

# Design

Give every gate a log directory outside the worktree (per gate id, bounded retention), pass it to dagrun, record it as log_path in the ledger row, and prune old directories.

# Acceptance Criteria

- Every gate ledger row names a log directory that holds each step's log.
- Retention is bounded and tested; deterministic tests.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
