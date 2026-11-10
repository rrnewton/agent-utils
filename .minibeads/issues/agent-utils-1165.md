---
title: "reply-count-broadcast-only: a main-channel reply counts toward its thread only when it appears in both places"
status: closed
priority: 2
issue_type: feature
assignee: mu-refactor
created_at: 2026-11-09T22:10:00.000000000+00:00
updated_at: 2026-11-09T22:10:00.000000000+00:00
---

# Description

The owner settled whether a reply that reaches the main channel counts toward its thread's reply count
(2026-11-09): "I think it should only if it appears in both places (slack implementation) other backends may put
it only in main".

So a reply counts toward the thread's reply count (the Reply button count, the way down, and the unanswered group)
only when it appears in BOTH the thread and the main channel, as with Slack's "also send to channel" broadcast,
which is both a thread reply and a main post. A reply posted ONLY to main does NOT count toward any thread. That
covers our Post to main (#1052 post-to-main) and any backend that places a reply only in the main channel. A
thread-only reply counts as before.

# Design

The #624 voice-page-modularity piece "reply counts + the way down + the unanswered group" was held for this
answer. The same lane moves that code word for word first, then applies this rule on top in a separate commit, so
the move stays reviewable as a pure move. Apply the rule wherever the count is derived (page, server or adapter),
using the provider's own marker for a broadcast reply rather than guessing from text.

Middle ground (lead, 2026-11-09, owner informed via liaison): framed by backend capability.
1. Where the backend HAS thread-plus-broadcast (Slack, its marker present): a reply counts toward
   Reply(N)/Thread(N)/tally/gather/unanswered only if it is in the thread, or in the thread AND
   broadcast to main. A main-only message with a reply_to pointer adds no Reply count and no circle.
2. Where a reply is ONLY a pointer, with no thread to also appear in (Discord-style native replies,
   any backend reporting no broadcast capability), pointer replies keep counting on Reply(N).
3. This page's own Post-to-main sends never count toward the quoted message's Reply(N).
4. The capability is explicit: `broadcast_supported` in the wire contract (additive, optional,
   sent only when true) plus per-message `MessageThread.broadcast`. Absent (an old server), the
   page falls back to today's behaviour minus its own Post-to-main sends.

# Acceptance Criteria

- A broadcast reply (in both the thread and main) counts toward its thread.
- A main-only post (Post to main, or a main-only backend) does not count toward any thread.
- A thread-only reply counts as before.
- Tests cover all three, in the layer that derives the count, plus one browser or page check of the visible count.

# Notes

Rule from the owner, 2026-11-09 (relayed by the lead coordinator). Follows #1052 post-to-main.

## Resolution

Delivered in this commit: `MessageThread.broadcast` carries the provider's also-sent-to-channel
marker (Slack `thread_broadcast`; absent and false everywhere else), `broadcast_supported` names
backends with the concept, and the page counts a main-channel reply naming a thread root toward
that thread only with the marker on such a backend — thread-only and broadcast replies count,
main-only and this page's Post-to-main sends do not, and an older server's data falls back as
above. Server, module and page tests cover the three cases and the visible count.
