---
title: 'server-side-search: Search further back, prioritising the channel''s crawl'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- search
depends_on:
  agent-utils-521: parent-child
created_at: 2026-10-28T01:28:05.700978227+00:00
updated_at: 2026-10-28T01:28:05.700978227+00:00
---

# Description

[opus 5.5] Part of #521 server-side-search (owner 2026-10-27 18:14; Q5/Q6 settled: a button prioritises our own crawl, no automatic upstream scans). Want: a "Search further back" button calling #602 history-settings-and-limits' POST /api/v1/channels/{id}/history/prioritize, polling GET .../history for live scope and count updates and re-running the server search as the archive grows, with Stop (DELETE). Stubbed behind a check until #602 lands.
