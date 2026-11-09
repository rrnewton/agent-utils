---
title: "reply-count-broadcast-only: a main-channel reply counts toward its thread only when it appears in both places"
status: open
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

# Acceptance Criteria

- A broadcast reply (in both the thread and main) counts toward its thread.
- A main-only post (Post to main, or a main-only backend) does not count toward any thread.
- A thread-only reply counts as before.
- Tests cover all three, in the layer that derives the count, plus one browser or page check of the visible count.

# Notes

Rule from the owner, 2026-11-09 (relayed by the lead coordinator). Follows #1052 post-to-main.
