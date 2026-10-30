---
title: 'generic-harness-fail-open: deliver to unknown harnesses when idleness cannot be told, behind safety guards'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
depends_on:
  agent-utils-710: related
created_at: 2026-10-30T21:05:11.467886958+00:00
updated_at: 2026-10-30T21:05:11.467886958+00:00
---

# Description

[opus 5.5] Follow-up of #710 chat-transcript-extraction (report section 8). Delivery fails closed today: it waits for Herdr's idle or done, and accepts 'unknown' only for Codex with an idle composer. Fail open for unknown harnesses instead, guarded by: the recorded harness process still in front (no shell), no approval or trust prompt on screen and no bare key answers (bracketed paste only), a quiescent screen and transcript, proof of receipt afterwards (transcript prompt entry or screen echo), one prompt in flight per pane, and a per-profile switch back to fail-closed. Test matrix over Herdr's 16 integrations plus Muse, agentterm and a scripted fake TUI.
