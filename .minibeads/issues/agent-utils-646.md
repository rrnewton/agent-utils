---
title: 'cache-shadow-thread-replies: classify thread replies in Slack newest-window shadow checks as expected'
status: open
priority: 4
issue_type: task
labels:
- vibe-talk
- cache
depends_on:
  agent-utils-633: discovered-from
created_at: 2026-10-28T13:58:59.687644942+00:00
updated_at: 2026-10-28T13:58:59.687644942+00:00
---

# Description

[opus 5.5] Moved from the coordinator's private backlog by #633 backlog-to-tracker (entry dated 2026-10-25). The message cache's shadow comparison for Slack newest windows counted thread replies as missing_rows (31 in one window), because Slack windows carry thread replies and those windows are never served from the cache. Wanted: classify that case as expected rather than WARN, and optionally raise the relist page size to remove listing gaps. Check first whether the shadow still runs this comparison now that serving is on.
