---
title: 'vt-timeline-latency: timeline requests take 40-65 s after a restart'
status: open
priority: 1
issue_type: bug
created_at: 2026-10-29T15:23:44.387424470+00:00
updated_at: 2026-10-29T15:23:44.387424470+00:00
---

# Description

Since a restart of the live Slack instance (no server code change), timeline requests
answer in 40-65 s instead of a few hundred ms, although `[message_cache] serve = true`.
Measured on the host with the read token:

- `GET /api/v1/channels/<C>/timeline?view=thread&thread_id=<ts>`: 43 s (2 messages, correct)
- `GET /api/v1/channels/<C>/timeline?view=flat&limit=30`: 65 s

The page gives up on these slow answers and shows replies as missing.

Find where the time goes and fix it in the server so a timeline request never waits on
upstream work when the cache can serve. Add a regression test.
