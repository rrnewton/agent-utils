---
title: 'vt-first-relist-backoff: pace the first relist after start so it does not hit an emptied rate bucket'
status: open
priority: 3
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-02T16:26:35.748628201+00:00
updated_at: 2026-11-02T16:26:35.748628201+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). ~7 min after each vibe-talk start, the first [ingest] relist hits a conversations.history bucket the startup reads just emptied (5 rate-limit backoffs each deploy). Delay or pace the first relist after startup. (lieutenant 2026-11-01; minor)
