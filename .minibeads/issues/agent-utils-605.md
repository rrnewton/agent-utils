---
title: 'server-side-search: a /search route under the serving rules, and the scope line'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- search
depends_on:
  agent-utils-521: parent-child
created_at: 2026-10-28T01:27:55.039913970+00:00
updated_at: 2026-10-28T01:27:55.039913970+00:00
---

# Description

[opus 5.5] Part of #521 server-side-search (owner 2026-10-27 18:14/18:17: own-server search first, upstream search deferred). Want: GET /api/v1/channels/{id}/search answering from the cache only where Sink::try_serve's gates allow (verified account, provider not suspended, channel or thread not in doubt); the page searches through it and shows "No upstream search · this page: back to DATE · server: back to DATE (complete / still expanding) · N messages searched", times in ET for the owner's instance.
