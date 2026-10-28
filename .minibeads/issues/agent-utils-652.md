---
title: 'sent-reply-joins-thread: a sent thread reply joins its thread at once, locally, not seconds later'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
depends_on:
  agent-utils-630: related
  agent-utils-621: related
created_at: 2026-10-28T14:53:37.722656369+00:00
updated_at: 2026-10-28T14:53:37.722656369+00:00
---

# Description

[opus 5.5] Owner 2026-10-28 10:53 ET: after #621 send-double-check (live since 10:25 ET), a reply sent in Threads view still sits as if it were a main-channel message (possibly with the Thread decorator) after the second tick turns green, until a later refresh several seconds on moves it into its thread. Expected (local-first, #630 state-sync-model): as soon as the send completes (or optimistically at send, marked pending), the message is placed in its owning thread locally, with no network round trip. Also check: the 10:50 ET send logged send_timing twice, 2 ms apart (op_key outgoing-mvs8eafh-2-r20h4dv): a duplicate log line, or the outcome delivered twice? Owner policy: land before #624 voice-page-modularity starts or after it finishes, never during.
