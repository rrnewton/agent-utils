---
title: 'chat-reply-off-turn-guard: hold a coordinator''s reply to an old owner request when it is answering lane chatter'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-11-02T19:35:24.789822325+00:00
updated_at: 2026-11-02T19:35:24.789822325+00:00
---

# Description

[opus 5.5] 2026-11-02 evidence: a Muse coordinator posted 14 replies under one owner request and 16 under another; most were one-line status updates written in answer to incoming lane (agentctl) or inter-team (agent-teams) messages, each wrapped as a reply block for the most recent open owner request, so lane chatter landed in the owner's topic threads. The coordinator wrote them deliberately (its committed messages were the reply blocks); the bridge posted them faithfully from the pane. A reply should answer the owner's turn. Asks: (1) when the coordinator writes a CHAT_REPLY block for a request while the prompt it is answering is a lane or agent-teams message (not the owner's request or a reply reminder for it), the bridge holds that reply (or posts it as a new root message, configurable) and reminds the coordinator of the rule; (2) warn the coordinator (and note in health) after N replies to one request (configurable, default e.g. 5); (3) tests with a fake coordinator answering a lane message with an old reply ID.
