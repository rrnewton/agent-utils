---
title: 'chat-transcript-primary: chat main-session trace: an opt-in mode that posts the agent''s non-reply transcript text to a daily trace thread'
status: open
priority: 2
assignee: mu-transcript@devbig014
issue_type: feature
labels:
- agentctl
depends_on:
  agent-utils-710: related
created_at: 2026-10-30T21:05:06.461345010+00:00
updated_at: 2026-10-30T21:05:06.461345010+00:00
---

# Description

[opus 5.5] Follow-up of #710 chat-transcript-extraction (report sections 4 and 5). An opt-in mode per agent: prompts typed with a one-line header and no reply instructions; assistant text from the transcript routed to the prompting thread (turn started by the owner message, or text after a mid-turn owner message until the next prompt), light routing tags (to a thread, to main, TUI only), recap and repeat filters, coalescing and splitting. Simulated volume on replayed transcripts: 1.7-3.6 messages/h after filters (6/h Claude, 29/h Muse unfiltered). Run side by side with fenced mode before relying on it.

[opus 5.5] 2026-11-02 (lead): assigned to mu-transcript@devbig014. It touches the chat bridge like #783: mu-transcript designs and builds behind an opt-in flag; cl-start's #783 slices land first and mu-transcript rebases over them; any file both need is coordinated through the two coordinators.

## Owner correction (2026-11-02): fences stay; scope is a main-session trace

[opus 5.5] Fences STAY: many threads need explicit CHAT_REPLY routing, and reply heuristics are unreliable. #714 becomes "chat main-session trace": an opt-in bridge mode that takes the agent's assistant text from its transcript, removes the CHAT_REPLY blocks (already posted as replies), and posts the rest to ONE dedicated daily trace thread (not the main channel), each turn labelled with the prompt it followed. Fenced replies are unchanged. The slug stays chat-transcript-primary; the lane on the other host re-plans to this.
