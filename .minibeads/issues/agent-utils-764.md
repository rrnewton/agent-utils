---
title: 'chat-subscription-discord: a Discord chat provider for bridges and the relay'
status: open
priority: 3
issue_type: task
labels:
- agentctl
created_at: 2026-11-02T16:26:35.767590457+00:00
updated_at: 2026-11-02T16:26:35.767590457+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). agentctl has no Discord chat provider yet; build rs/chat-subscription-discord (bot token, gateway + REST, allowed_senders/relay_senders, 2000-char posts -> attachments/parts) so the relay works on the owner's OSS Discord install (private relay channel created, both Discord bots invited). Hand to the OSS team when tested. Plan: mu-relayfix continues to it right after #722 fix lands.
