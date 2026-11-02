---
title: 'chat-stream-child-resources: the bridge''s events-stream child had 745 threads and 886 MB RSS minutes after start'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-02T18:18:28.748666834+00:00
updated_at: 2026-11-02T18:18:28.748666834+00:00
---

# Description

[opus 5.5] 2026-11-02 on one host: a chat bridge's provider events-stream child process had 745 threads and 886 MB RSS 5.5 min after the bridge started. A sampling trend over an hour will be attached. Find whether it is a thread-per-connection or per-subscription leak, an unbounded buffer, or expected; cap threads and memory, and surface the child's resource use in chat health.
