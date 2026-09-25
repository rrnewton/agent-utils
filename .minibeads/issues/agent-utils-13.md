---
title: 'voice-chat-tools: let the internal agent read configured chat'
status: in_progress
priority: 0
issue_type: feature
assignee: opus-5.5
labels:
- vibe-talk
- voice
- chat
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-25T03:55:15.307829973+00:00
updated_at: 2026-09-25T05:09:54.970802427+00:00
claimed_at: 2026-09-25T05:09:54.970802427+00:00
claimed_until: 2026-09-27T05:09:54.970716288+00:00
---

# Description

Make the provider-neutral conversational voice interface expose the chat read, search, and optional write capabilities to the selected voice provider. Prove that the internal agent can summarize real configured chat messages. Writes must remain unavailable without write scope and must occur only on an explicit user request.

# Acceptance Criteria

From the live voice UI, the internal agent lists its tools and summarizes real chat content; an explicit send works with write scope and no implicit send occurs.
