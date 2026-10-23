---
title: 'replication-c5-post-keys: persist the send log so a repeat after a restart posts nothing'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- replication
created_at: 2026-10-23T19:07:44.704677730+00:00
updated_at: 2026-10-23T19:07:44.704677730+00:00
---

# Description

[opus 5.5] #454 replication-model C5. Related: #468 send-dedupe, whose persisted keys stay in this lane. The send log (post_keys, #448 send-dedupe) remembered landed posts in memory only, so a restart answered a repeat of a landed post with post_unknown and a retry could post it again. File each settled outcome in the store (post_keys table: key, the send log record as JSON, at_ms), bounded by 24 h and 4096 like the memory, purged with the store, and read back at start.

# Acceptance Criteria

A restart on the same store answers a repeat from the store and posts nothing; both backends bounded and purged; migration and rollback documented.
