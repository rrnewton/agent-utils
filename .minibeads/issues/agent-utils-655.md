---
title: 'send-undo-in-flight: Esc on the send card before the second tick cancels the send'
status: open
priority: 3
issue_type: feature
labels:
- vibe-talk
depends_on:
  agent-utils-654: blocks
created_at: 2026-10-28T15:13:15.938082826+00:00
updated_at: 2026-10-28T15:13:15.941902752+00:00
---

# Description

[opus 5.5] Owner 2026-10-28 11:00 ET: Esc on the Sending card before the second tick returns to the composer with the text; the server drops the op before it reaches Slack. First delete/undo-style operation; outcomes cancelled / too-late (offer delete) / unknown. Stage after the refactor and after the formal state-model round, then decide if it is still possible.
