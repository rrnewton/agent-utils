---
title: 'chat-reply-splitting: answer separate topics in separate chat replies'
status: closed
priority: 2
issue_type: feature
labels:
- agentctl
- chat
created_at: 2026-10-22T17:20:51.434840695+00:00
updated_at: 2026-10-22T17:32:50.425854240+00:00
closed_at: 2026-10-22T17:32:50.425854100+00:00
---

# Description

The owner asked, over the chat bridge, for several medium-sized replies on separate topics rather than one large message combining them. A request's reply ID can be used many times, so one request maps to a series of replies: split by time (acknowledgement, progress updates, result) and by topic (a message asking three things gets three replies, one per topic).

Deliver: general guidance in skills/agentctl-chat/SKILL.md; a short hint in the Rust bridge's request prompt so agents without the skill loaded see it; and the same in the chat quickstart and userguide where they describe replying.
