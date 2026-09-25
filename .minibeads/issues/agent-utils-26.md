---
title: 'validation-flakes: eliminate environment and deadline failures'
status: open
priority: 0
issue_type: task
labels:
- validation
- reliability
depends_on:
  agent-utils-21: parent-child
created_at: 2026-09-25T04:10:41.859585672+00:00
updated_at: 2026-09-25T11:39:54.964206261+00:00
---

# Description

[gpt-5.6-sol] Make validation deterministic across supported local and CI environments, including known deadline, PATH, and process-cleanup failures.

# Acceptance Criteria

Known deadline and environment flakes are reproduced or classified; tests use explicit prerequisites and robust synchronization; repeated targeted and full runs have no unexplained failures.
