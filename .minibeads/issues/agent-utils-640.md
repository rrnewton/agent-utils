---
title: 'wrkviz-invented-key-retry: summarize stops when the model invents a phase key'
status: open
priority: 3
issue_type: task
labels:
- wrkviz
depends_on:
  agent-utils-633: discovered-from
created_at: 2026-10-28T13:58:35.620964949+00:00
updated_at: 2026-10-28T13:58:35.620964949+00:00
---

# Description

[opus 5.5] Moved from the coordinator's private backlog by #633 backlog-to-tracker (entry dated 2026-10-26). wrkviz summarize aborts when a model answer names a phase key that does not exist. Wanted: treat an invented key like a left-out phase: drop it, retry the missing phases, and keep the paid work. Verify first against #568 wrkviz-summarize-budget-plan, which reworked summarize after this was noted.
