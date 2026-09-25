---
title: 'read-aloud-latency: measure and reduce time to first audible audio'
status: open
priority: 0
issue_type: task
labels:
- vibe-talk
- audio
- performance
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-25T01:32:02.842014199+00:00
updated_at: 2026-09-25T01:33:45.750034211+00:00
---

# Description

[gpt-5.6-sol] Add enough content-free timing evidence to separate message preparation, provider WebSocket connect, session readiness, first PCM, transfer, and browser playback. Existing logs cannot reconstruct a completed request and the current conversation backend warm-up is a no-op.

# Acceptance Criteria

One real read produces durable phase timings without message content or credentials; the dominant stage is identified; a regression test covers timing instrumentation; a justified latency improvement is implemented or a blocker is documented.

# Notes

The completed request could not be reconstructed: retained server logs contain no phase timing and the browser records none. The streaming path opens a new provider WebSocket per read, waits for session readiness, and does not measure first PCM or audible playback. Add content-free phase metrics before optimizing.
