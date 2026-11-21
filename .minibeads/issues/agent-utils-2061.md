---
title: 'wrkslots-image-status-mounts: image status must be read-only'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T11:45:00-05:00
updated_at: 2026-11-21T11:45:00-05:00
---

# Description

`wrkslots image status` re-mounted slot images it had just unmounted (new loop devices), so a status command has a mounting side effect and leaks mounts.

# Design

Status reports mount state without mounting anything; whatever needed the ensure-mount keeps it out of status. A test shows status on an unmounted slot leaves it unmounted.

# Acceptance Criteria

- The behaviour described under Design, covered by a deterministic regression test (condition-based, no wall-clock budgets).

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
