---
title: 'voice-full-text-catchup: hand the voice model the full text of recent messages for a catch-up'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- voice
depends_on:
  agent-utils-436: parent-child
created_at: 2026-10-22T17:26:16.915485386+00:00
updated_at: 2026-10-22T17:26:16.915485386+00:00
---

# Description

[opus 5.5] Asked to summarise recent progress, the voice model should read the full text of recent messages (within a budget, noise rules applied, ids kept as handles) and reason from it, not from 160-character digest lines. The catch-up must answer inside the voice runtime tool deadline, from this server own held copy where the chat service is slow.

# Acceptance Criteria

recent_activity carries full message text (optionally for one channel) within a byte budget; digest_channel is no longer offered to a model; descriptions and the system prompt steer catch-ups to it; every model-facing read answers before the runtime deadline; tests for each.
