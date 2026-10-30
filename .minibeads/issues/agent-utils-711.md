---
title: 'chat-auto-reply-fallback: post the final text of an owner-prompted turn that sent nothing to chat'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
depends_on:
  agent-utils-710: related
created_at: 2026-10-30T21:04:44.147957718+00:00
updated_at: 2026-10-30T21:04:44.147957718+00:00
---

# Description

[opus 5.5] Follow-up of #710 chat-transcript-extraction (report section 9 step 2). When a turn that carried an owner chat message ends having sent nothing to chat (no reply block, no chat reply), post that turn's final assistant text from the transcript to the message's thread, marked as an automatic fallback. Measured in the replayed coordinator transcripts: about once every two days per coordinator, including the owner's own 'don't reply yet' holds, which a fallback must respect.
