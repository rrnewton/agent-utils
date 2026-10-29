---
title: 'chat-requests-unbatched: owner messages are never batched; working-pane batching off by default'
status: closed
priority: 0
issue_type: bug
created_at: 2026-10-29T20:32:31.572862183+00:00
updated_at: 2026-10-29T21:16:08.350114992+00:00
closed_at: 2026-10-29T21:16:08.350114852+00:00
---

# Description

Follows #697 send-muse-while-working (landed 7355a4e3).

## Bug (live, 2026-10-29 16:25 ET)

#697 made a drain into a working pane combine its ordinary backlog into one
"N queued messages:" prompt. An owner chat-bridge request was folded into such a
"4 queued messages:" prompt with three agent notes. The owner saw a giant batch in the
agent's TUI rather than their own message, and read the delivered reaction as false (the
agent had in fact read and answered the request). Muse also caps long pastes in its TUI
as "... output capped", so batches are unreadable there.

## Owner rules

- A message from the owner (any chat-bridge request, or console input sent by a human at a
  terminal) is never folded into a batch. It is delivered on its own, as its own visible
  prompt, ahead of any queued agent backlog.
- Owner, 16:32 ET: "I don't actually like batching. Please turn it off in the next cut. It's
  OK if it stays as an internal flag, but off for now."

## Fix

- Working-pane backlog batching is off by default and kept behind an internal drain option.
  By default every queued prompt is delivered individually, oldest first.
- A drain types owner prompts first (stable order), each individually; the agent backlog
  follows.
- With the internal option on, a batch never contains an owner prompt, so the bridge's
  delivered reaction is only ever set by that prompt's own submission.
- The rest of #697 stays: the adopted-Muse fast path and urgent sends jumping the queue.
