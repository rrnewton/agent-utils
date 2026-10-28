---
title: 'chat-bridge-permalinks: prompts carry permalinks; one shared thread numbering with vibe-talk (design first)'
status: open
priority: 3
issue_type: task
labels:
- agentctl
created_at: 2026-10-28T15:37:24.096105057+00:00
updated_at: 2026-10-28T15:37:24.096105057+00:00
---

# Description

[opus 5.5] Owner 2026-10-28 11:15-11:20 ET. (1) Bridge prompts print ready permalinks (message and thread) so the coordinator links earlier answers; the reply-id aliases stay internal and are never shown to the owner. (2) chat reply/sent accept the Slack message id as well as the alias. (3) When combined with vibe-talk, use the same thread numbering: canonical id = Slack ts; optional short per-channel label assigned by vibe-talk and PUSHED to the bridge or kept in a local shared file. Rule: no network call on the bridge's delivery path; labels only from local state, falling back to permalinks. Design doc for owner approval before any build.
