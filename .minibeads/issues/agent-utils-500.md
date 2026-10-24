---
title: 'sent-receipts-pile-up: Sent receipts of replies pile up at the foot and survive a reload'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- send
created_at: 2026-10-24T19:47:03.193944045+00:00
updated_at: 2026-10-24T20:07:53.049072582+00:00
closed_at: 2026-10-24T20:07:53.049072461+00:00
---

# Description

[opus 5.5] The owner, 2026-10-24, on the Slack instance, desktop, Threads view: "Some weird bug where messages I sent stack up in the 'Sent.' cards at the bottom of the screen, and they survive even the page restart." Five "You · Reply … Sent." receipts of thread replies, each with a Thread chip, piled at the foot of the cards above the main box long after they posted.

Cause: since #466 reply-promotes-thread a sent receipt retires only when a READ carries every part (the R9 interim rule), and confirmed receipts are saved in the outbox. A reply into a thread the page does not read (Main, the Threads view's cards) is never carried by a read, so its receipt never retires, and every reload restores it. #473 replication-c7-server-versions now orders copies by the server's versions, and #466's grace keeps the page's own post against unversioned pages, so the receipt can be a display aid again.

# Acceptance Criteria

A confirmed send's receipt retires once the page holds the message (any read, the stream's copy, the store's copy from the post's answer) after a short showing, or after a bound once confirmed. Confirmed receipts are never saved, and those saved by earlier builds are dropped on load. A reply sent from among the cards keeps its receipt under the row it answered (#467). Page tests reproduce the owner's scenario: several thread replies from the Threads view, a reload, no receipts, messages still held.
