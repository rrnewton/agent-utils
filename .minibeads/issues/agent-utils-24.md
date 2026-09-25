---
title: 'validation-parallelism: execute the full contract efficiently'
status: open
priority: 0
issue_type: task
labels:
- validation
- performance
depends_on:
  agent-utils-21: parent-child
created_at: 2026-09-25T04:10:40.477016914+00:00
updated_at: 2026-09-25T11:39:54.964327414+00:00
---

# Description

[gpt-5.6-sol] Turn the complete portable validation contract into a profiled dagrun graph and tune representative outer plus inner concurrency from measured sweeps.

# Acceptance Criteria

A clean full validation completes under 10 minutes on the target host; plans expose the critical path and resource choices; independent work overlaps safely; a measured lifecycle outer-width sweep and cold Cargo inner-width sweep identify stable knees without excessive CPU; the final full-graph profile shows remaining utilization and critical-path limits; selective runs remain substantially shorter.
