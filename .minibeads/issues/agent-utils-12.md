---
title: 'live-transcript-latency: render speech text promptly'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- voice
- performance
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-25T03:55:15.304766957+00:00
updated_at: 2026-09-25T03:55:15.304766957+00:00
---

# Description

Trace both user and agent transcript events from provider receipt through normalization, storage, network delivery, and DOM rendering. Remove avoidable buffering so text appears with the corresponding speech, while retaining correct final transcript state.

# Acceptance Criteria

User and agent text updates promptly during a live call, phase timing identifies any remaining external delay, and browser tests cover streaming and final events.
