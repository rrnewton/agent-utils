---
title: 'send-settle-in-task: record a send''s outcome even when its caller has gone'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-14T03:02:06.722525898+00:00
updated_at: 2026-10-19T09:07:33.427601105+00:00
closed_at: 2026-10-19T09:07:33.427600945+00:00
---

# Description

Found in review of #269 approval-card-inline / #270 voice-transcript-markers after #278 send-order. The SendTicket/DirectTicket is owned by the request future, while the post itself now runs in its own task (ops.rs reply_scoped). (a) A caller that disconnects mid-post: the post almost certainly lands, but the send memory, the frozen card and the replay say 'not confirmed'. (b) A caller that disconnects while waiting for the destination's lane: nothing is posted, yet the words are filed as an unconfirmed send. Both are honest but imprecise. Fix: move the settle into the spawned post task (pass the ticket or a settle closure into Posting) so the outcome is recorded whether or not anyone waits for the answer; tests for both disconnect points.
