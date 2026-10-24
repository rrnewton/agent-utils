---
title: 'reply-promotes-thread: a reply to a plain main message shows in its new thread at once, in every view'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- send
- threads
depends_on:
  agent-utils-448: related
created_at: 2026-10-23T17:55:05.819317921+00:00
updated_at: 2026-10-24T00:50:18.980483700+00:00
closed_at: 2026-10-24T00:50:02.697341765+00:00
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

# Notes

[opus 5.5] Delivered on main in 385abd6e (the vanished reply) and 9e8b1aa1, 6326e6be, c49acff9 (the overlay, with #467 and review fixes). The vanished reply was reproduced before fixing: the stream copy of the reply asked for a newest page of All, sent after the post but answered from older state, which carried the message answered with a count of none and so took the reply out as deleted upstream (replication model R9). Interim fix, marked for C7: the page keeps its own post for 10 minutes against absence (a heard removal still takes it out) and retires a sent receipt only on a read. Overlay: a send still in the outbox counts in the thread it goes to, keyed by the thread record id, derived at every pass and never written into the store; the message answered reads Thread(N) in its colour, the Threads view (one column and the right column) has a card, the thread shows the send; a send not sent keeps it all with Retry until dismissed; the card stays once the reply lands (a held reply holds it); an echoed send is not counted twice. Tests: page tests per view (Main, Threads view, the thread, two columns, sent, not sent, echo) and Chromium (tests/inline_reply_browser.py promotes walk on a Slack-like fake). Filed as #477 reply-promotes-thread: hue seeding at promotion (F5; the reply keeps its fallback colour until the server colours the thread) and R2 placement of a failed reply whose root was removed in the gap in Main (owner answer 2; a removed root refuses the send at placing, which says so on the row at the foot). Residual: the parts a 207 landed get no grace (no part ids in a 207).
