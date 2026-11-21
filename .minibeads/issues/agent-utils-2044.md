---
title: 'gate-refuses-slot-root-cwd: gate.py must not silently gate a slot root''s stale branch'
status: open
priority: 2
issue_type: bug
labels:
- gate
created_at: 2026-11-21T11:45:00-05:00
updated_at: 2026-11-21T11:45:00-05:00
---

# Description

gate.py or validate run from a slot root that hosts nested worktrees tests that slot root's own old branch instead of the lane's change; it happened three times, the latest on 2026-11-21.

# Design

Refuse to run from a directory that hosts nested lane worktrees unless --allow-slot-root is given, and print the head commit and branch prominently at start.

# Acceptance Criteria

- The behaviour described under Design, covered by a deterministic regression test (condition-based, no wall-clock budgets).

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
