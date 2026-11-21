---
title: 'gate-memory-floor: do not start gates or heavy jobs below a free-memory floor'
status: open
priority: 2
issue_type: bug
labels:
- gate
created_at: 2026-11-21T11:45:00-05:00
updated_at: 2026-11-21T11:45:00-05:00
---

# Description

The owner asked for host-wide coordination by memory: heavy work should not start when free memory is below a floor, rather than waiting for a quiet machine.

# Design

A gate.py defer key for a MemAvailable floor (observable in the gate log and ledger), usable by both teams.

# Acceptance Criteria

- The behaviour described under Design, covered by a deterministic regression test (condition-based, no wall-clock budgets).

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
