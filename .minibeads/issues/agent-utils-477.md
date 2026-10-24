---
title: 'reply-promotes-thread: seed the new thread colour at promotion, and draw a reply to a removed root in its gap'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- threads
depends_on:
  agent-utils-466: related
created_at: 2026-10-24T00:50:18.976384803+00:00
updated_at: 2026-10-24T00:50:18.976384803+00:00
---

# Description

[opus 5.5] Follow-up to #466 reply-promotes-thread (delivered through c49acff9), for the two parts it left:
- F5 (replication model §5.3): at promotion, pick the thread colour as the page picks a fallback and seed it at once (POST .../thread-hues, first writer wins at the server), so the new thread does not change colour when the server first colours it; take the server answer as final.
- R2, owner answer 2 (2026-10-23): a failed reply whose parent was removed upstream goes in the gap in Main where the message was, with the outbox foot as the fallback. Today the placing read refuses it and its row says so at the foot (#315 removed-reply-target).
- #467 inline-send-in-place does not place rows under the rows unpacked under the Threads view cards in two columns (they go to the foot); include them if the owner wants it there too.
