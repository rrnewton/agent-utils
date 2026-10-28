---
title: 'chat-rescan-incremental: make the Slack plugin''s history rescan incremental'
status: open
priority: 2
issue_type: task
labels:
- agentctl
- chat
- slack
depends_on:
  agent-utils-633: discovered-from
created_at: 2026-10-28T13:58:51.715714473+00:00
updated_at: 2026-10-28T13:58:51.715714473+00:00
---

# Description

[opus 5.5] Moved from the coordinator's private backlog by #633 backlog-to-tracker (owner, 2026-10-28). Every 90 s the Slack subscription plugin re-lists 24 hours of channel history (up to 5 pages) and up to 8 threads at 2 pages each, sharing the Slack budget with vibe-talk. Wanted: an incremental pass (oldest = last pass minus a grace window, one call; threads only when their latest_reply advanced) plus an hourly full sweep, with the same guarantee that file messages the live stream never carries still arrive.
