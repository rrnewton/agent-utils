---
title: 'relay-rescan-settle-latency: relay posts wait up to about 11.5 min after a bridge restart'
status: open
priority: 3
assignee: mu-mirror@devbig014
issue_type: task
labels:
- agentctl
created_at: 2026-11-02T16:26:35.786357109+00:00
updated_at: 2026-11-02T16:26:35.786357109+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). Slack plugin starts rescans only after stream catch-up (heartbeat/event/SETTLE_FALLBACK 10 min); after a bridge restart on a quiet tail relay latency is up to ~11.5 min, then up to 90 s per post. relay listen would fix. (014 lieutenant 2026-11-02)

[opus 5.5] 2026-11-02: reassigned to mu-mirror@devbig014 as its third item after #770 and #782: a relay-only fast poll in the Slack plugin (relay posts from the shared bot never arrive on the live stream, so today they wait for the 90 s rescan and its settle).
