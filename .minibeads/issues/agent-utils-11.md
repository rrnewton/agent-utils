---
title: 'voice-connect-latency: make call startup measurable and fast'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- voice
- performance
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-25T03:55:15.301955780+00:00
updated_at: 2026-09-25T03:55:15.301955780+00:00
---

# Description

Measure and reduce the delay from pressing Start a new call through session acquisition, browser WebSocket connection, provider readiness, first greeting transcript, and first audible greeting. Emit only content-free phase timing. Test the live deployment as well as deterministic local cases.

# Acceptance Criteria

A live call records each phase, the dominant delay is identified and reduced or isolated, and the greeting appears and is audible.
