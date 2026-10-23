---
title: 'reply-promotes-thread: a reply to a plain main message shows in its new thread at once, in every view'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- send
- threads
depends_on:
  agent-utils-448: related
created_at: 2026-10-23T17:55:05.819317921+00:00
updated_at: 2026-10-23T17:55:05.819317921+00:00
---

# Description

[opus 5.5] The owner: "the message I was replying to was a main-channel root message that was not yet a thread. After I sent, I expected the main-channel message to be promoted to the root of a new thread, and that didn't visually happen, not locally and not quickly. Instead my sent message disappeared, and I didn't see it anywhere on the screen."

Design (replication model §5.3, C4, with the corrections relayed 2026-10-23):
- First reproduce the vanished reply in the page tests and in Chromium before naming a cause. Likeliest R9: the sent receipt is retired on the stream echo, then a newest page answered from older state drops the part. If confirmed, ship C7's interim receipt fix (keep a sent receipt until a read sent after the post carries it; do not drop an own posted copy on absence alone shortly after its post).
- A derived overlay at projection time, never a mutated stored copy: thread keyed by X.thread.id (the ts string on Slack, not X.id); reply count = max(U_count + ops sent after that copy was read, known replies + unresolved ops); only where replies are thread replies.
- The outgoing reply drawn in place in thread X in Main, All, the Threads view card, the thread itself and two columns; consistent across view switches.
- A failed reply stays held with Retry in the thread, so its display persists until dismissed. Owner, answer 2: a failed reply whose parent was removed upstream goes in the gap in Main where the message was, with the outbox foot as the fallback. Owner, answer 6: a root deleted upstream with replies remaining keeps its thread under a "message removed" root.
- Hue picked and seeded at promotion; durable effects (Done, the provider cursor) only after the post is confirmed.
Tests: page and Chromium, per view.
