---
title: 'wrkslots-partial-status-claims-consistent: a partial status must not claim the registry is consistent'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T11:45:00-05:00
updated_at: 2026-11-21T11:45:00-05:00
---

# Description

When the scan bound is exceeded, a partial `wrkslots status` still prints registry_storage=CONSISTENT inconsistencies=0, while a full scan of the same tree finds 48 inconsistencies. That is an absence claim from an incomplete scan.

# Design

A partial run prints UNKNOWN for registry storage and inconsistencies and says it is partial; it never asserts absence. Both editions.

# Acceptance Criteria

- The behaviour described under Design, covered by a deterministic regression test (condition-based, no wall-clock budgets).

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
