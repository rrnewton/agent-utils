---
title: 'bulk-reads: bulk and time-range reads in the chat provider traits'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-14T10:01:57.326222933+00:00
updated_at: 2026-10-14T10:01:57.333144413+00:00
closed_at: 2026-10-14T10:01:57.333144313+00:00
---

# Description

[opus 5.5] remote-ops-audit W10-W14: five bulk and time-range reads on the chat provider traits (fetch_range, fetch_messages, channel_heads, fetch_recent_many, fetch_threads), each with a default that makes exactly today's reads at today's cost, ReadCaps saying per channel which are native, native Slack, Discord and bridge answers (bridge ones behind settings that are off by default), and callers that use a read only where it is native. Thread naming does not batch its reads: under #286 naming-budget each thread is named only from a read made at its own turn.
