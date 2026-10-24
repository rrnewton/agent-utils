---
title: 'test-time-agent-utils: profile and shorten the gate''s test suites outside vibe-talk'
status: open
priority: 2
issue_type: task
labels:
- validation
- performance
created_at: 2026-10-24T12:07:05.365267200+00:00
updated_at: 2026-10-24T12:07:05.365267200+00:00
---

# Description

[opus 5.5] The owner asked for a round of test-suite optimization: anything over 10 minutes deserves inspection, and pruning where the regression signal is redundant or out of proportion to its time. This issue covers every suite outside vibe-talk (wrkslots, agentctl, dagrun, herdr-run, tick-hub, pr-landing-planner, wrkviz, packaging, site, cross/ checks, the Python shards); test-time-vibe-talk covers vibe-talk. Baseline full-graph gate on 2026-10-24: 830.3 s wall, 11,978 CPU-s. The eleven wrkslots Python steps share lifecycle-io: 5 and sum 3,794 s of wall, so that pool sets about 760 s of it.

# Acceptance Criteria

Every step and case over 10 minutes of wall or CPU, and the 20 slowest cases, are listed with measured numbers in ai_docs/transient/2026-10-24-test-time-agent-utils.md, with before and after per step and for the whole gate. Each prune has a written justification naming what still covers it; debatable ones are listed for the owner.
