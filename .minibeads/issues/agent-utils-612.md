---
title: 'view-stability: whatever the reader is looking at never jumps away when a thread re-sorts'
status: closed
priority: 1
issue_type: bug
assignee: cl-viewstab
created_at: 2026-10-28T04:17:34.797939752+00:00
updated_at: 2026-10-28T05:41:12.866615629+00:00
closed_at: 2026-10-28T05:41:12.866615489+00:00
---

# Description

[opus 5.5] Owner 2026-10-27 20:45 PDT: "I just experienced a screen jump. I was in threaded view reading a message and it jumped off the screen. It was probably bc of some new message causing a thread to jump to the bottom. But we should ALWAYS prioritize view stability. Whatever I'm looking at should not jump away."

Reproduced in Chromium (vibe-talk/web, thread list in the "list" layout at 393x852 touch and 1280x800): with the reader halfway up the thread list, a reply into the thread whose card is half off the top of the screen moves that card to the foot, and the cards held the reader by that one card (`cardAnchor`/`placeCardAnchor`) and followed it: every card the reader was reading went, 755px at 393x852, 245px at the desk in a scratch walk, 1496px and 722px in the gate walk. Main, All and an open thread held in every case tried (live post, reply above and below, edit far above, poll, summary change). The fake-DOM suite also found the card half off the top, grown by an edit or a summary, pushing every card shown whole down by its growth (180px), since it was the anchor.

Wanted, as one rule rather than per-path patches: a place in a list notes the rows on screen and each card's neighbours; a card standing beside neither of its old neighbours has moved and is not followed; the reader is held by the first row on screen, shown whole first, that stayed. Only a reader at the foot follows. Same for the one-column Threads list (`#thread-list`, `channelReadPosition`/`settleAfterRead`). Page tests with negative controls; a Chromium walk at 393x852 and 1280x800.
