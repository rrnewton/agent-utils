---
title: 'validation-selection: route each change to affected tests'
status: open
priority: 0
issue_type: task
labels:
- validation
- selection
depends_on:
  agent-utils-21: parent-child
created_at: 2026-09-25T04:10:39.074004671+00:00
updated_at: 2026-09-25T11:39:54.964385021+00:00
---

# Description

[gpt-5.6-sol] Replace coarse directory buckets with dependency-aware ownership and close known selection gaps without weakening unknown-path fail-safe behavior.

# Acceptance Criteria

Documentation-only and single-tool changes select only relevant checks plus hygiene; multi-area changes select the union and reverse dependencies; every tracked path and test has an owner; planner output explains every selection; local and CI consume the same plan.
