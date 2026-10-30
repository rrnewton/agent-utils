---
title: 'chat-owner-ask-relay: relay requests for the owner''s decision that agents write only to their TUI'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
depends_on:
  agent-utils-710: related
created_at: 2026-10-30T21:04:55.811552319+00:00
updated_at: 2026-10-30T21:04:55.811552319+00:00
---

# Description

[opus 5.5] Follow-up of #710 chat-transcript-extraction (report section 9 step 3). Text written in turns started by agents, timers or the watcher that asks the owner for a decision or approval ('waiting on your OK', 'your decisions are still open') stays in the TUI today: up to 3-5 a day per coordinator in the replayed transcripts. Detect such text in the transcript and post it to the bridge's main channel, rate-limited and de-duplicated against what was already posted (one Muse coordinator repeated the same open-decision list every hour).
