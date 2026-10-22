---
title: 'threads-main-own-sent: the owner''s message just sent to the main channel shows among the Threads view''s main rows'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-22T15:21:14.720695056+00:00
updated_at: 2026-10-22T15:21:14.720695056+00:00
---

# Description

[opus 5.5] The owner, 2026-10-22, on a phone: "even when I've toggled Main on in thread list View on mobile. I still don't see the message that I just sent to the main channel." Reproduced in Chromium (#419 threads-view-main-messages): with Show read or Collapse read the message sent from the foot box appears among the main rows at once; under Hide read it never does. Cause: the main rows are rebuilt through mainRowMessages, whose Hide read filter keeps only messages still to do, and the owner's own messages count as read (markOwnRead); Main shows the same message only because its live append skips the filter, until its next redraw. Second hole: the outbox receipts (#outgoing-log: Sending..., a failed or unconfirmed send) stay in the left column, hidden behind the cards, so in the Threads view a send on a slow network showed nothing until it landed.

# Design

A message sent from this page (the outbox's confirmed parts) is kept by the read filters for the rest of the page's life, in Main, All, a thread and the Threads view's main rows alike (sentFromHere, in readFilterKeeps and mainRowMessages), so it never vanishes as it is sent. The outbox receipts move to the foot of the cards with the main channel's box (placeMainCard), above it as in Main.

# Acceptance Criteria

Threads view, Main channel on, Hide read: the own message sent from the foot box (and from Main) appears among the main rows at once, at a desk and on a phone, and its receipt shows above the foot box while sending; with Main channel off it is not among the cards (negative control); Show read unchanged. Page tests; Chromium at 1280x800 and 393x852.
