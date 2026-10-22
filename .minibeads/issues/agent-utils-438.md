---
title: 'voice-owner-identity: know which messages are the owner''s and never read them back to him'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- voice
depends_on:
  agent-utils-436: parent-child
created_at: 2026-10-22T17:26:17.887356752+00:00
updated_at: 2026-10-22T20:00:51.349490508+00:00
closed_at: 2026-10-22T20:00:51.349490358+00:00
---

# Description

[opus 5.5] The voice assistant kept reading the owner his own messages. The configuration should name the owner and the accounts that are his, tool output should mark his messages, and the system prompt should say never to read them aloud unless he asks, using them only as context.

# Acceptance Criteria

[owner] section (name, extra account ids) with fallbacks to each provider owner_user_id and this server own posting account; every model-facing message line marks the owner; prompt rule; documented in vibe-talk.example.toml and the userguide; tests.
