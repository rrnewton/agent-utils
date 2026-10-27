---
title: 'headless-log-rotation: rotate the headless event log and tail across segments'
status: closed
priority: 2
issue_type: feature
labels:
- agentctl
- headless
depends_on:
  agent-utils-565: parent-child
created_at: 2026-10-27T17:37:11.327275815+00:00
updated_at: 2026-10-27T18:14:06.500389751+00:00
closed_at: 2026-10-27T18:14:06.500389601+00:00
---

# Description

[opus 5.5] Child of #565 headless-coordinator. One wrapper generation writes one events/<generation>.jsonl that grows without bound, and tail reads whole files. Rotate the current generation into segments at a size bound, keep seq continuous, prune by age and total size across segments, and make tail, --follow, --at and the writer's restart repair read across rotated segments.

# Acceptance Criteria

A long generation rotates at the bound; tail --follow crosses a rotation without losing or repeating events; restart after a crash mid-rotation keeps seq; tests.
