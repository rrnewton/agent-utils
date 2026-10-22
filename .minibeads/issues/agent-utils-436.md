---
title: 'voice-tool-audit: audit the chat tools and instructions the voice agent is given'
status: closed
priority: 1
issue_type: epic
labels:
- vibe-talk
- voice
created_at: 2026-10-22T17:26:15.898968128+00:00
updated_at: 2026-10-22T20:00:51.352694931+00:00
closed_at: 2026-10-22T20:00:51.352694821+00:00
---

# Description

[opus 5.5] The owner reported from the car (2026-10-22): the deployment-managed voice preview looped one sentence aloud, a catch-up request reached digest_channel which timed out (several times), and the assistant kept reading his own messages back to him. Audit every tool the voice agents are given (descriptors, arguments, real output and size), the instructions each backend gives the model, and fix what is ours. Working note: ai_docs/transient/2026-10-22-voice-tool-audit.md.

# Acceptance Criteria

Audit note landed; children voice-full-text-catchup and voice-owner-identity closed; the private-stack TTS loop written up with evidence and a proposed mitigation.
