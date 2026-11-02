---
title: 'relay-rescan-settle-latency: relay posts wait up to about 11.5 min after a bridge restart'
status: open
priority: 3
issue_type: task
labels:
- agentctl
created_at: 2026-11-02T16:26:35.786357109+00:00
updated_at: 2026-11-02T16:26:35.786357109+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). Slack plugin starts rescans only after stream catch-up (heartbeat/event/SETTLE_FALLBACK 10 min); after a bridge restart on a quiet tail relay latency is up to ~11.5 min, then up to 90 s per post. relay listen would fix. (014 lieutenant 2026-11-02)
