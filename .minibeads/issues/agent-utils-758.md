---
title: 'vt-deploy-drain: hand over the listening socket so a deploy costs no front-door downtime'
status: open
priority: 3
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-02T16:26:35.744037020+00:00
updated_at: 2026-11-02T16:26:35.744037020+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). each a host deploy costs ~30-40 s of front-door downtime: the old process's graceful shutdown waits for streaming connections (SSE/websocket) up to its deadline while the new one can't bind. Hand over faster: close streams with a reconnect hint, shorter drain, or socket handoff (systemd socket activation). (lieutenant 2026-11-01)
