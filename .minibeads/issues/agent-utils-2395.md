---
title: 'vibe-talk-reply-box-covers-last-card: Reply on the last card draws its box over the very message being answered'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-27T13:20:00-05:00
updated_at: 2026-11-27T13:20:00-05:00
---

# Description

The owner, 2026-11-27 12:39 PM ET, verbatim:

"In my OSS deployment I noticed that clicking reply on a main channel message (which itself was a promote-to-main reply to an EARLIER thread message) put me in the reply box but with bad scroll so my reply box is COVERING half of the message I was replying too. ... After ANOTHER message appeared in the All channel -- so that the target of the reply was no longer the LAST card in the history -- then the reply button places correctly. Please find and fix this bug and I'll tell the OSS team to redeploy"

Cause, reproduced locally (fake API, Chromium, phone 412x915 and desk 1280x800):

- `dockReplyAtFoot` (#2063 vibe-talk-quote-reply-view-stability) stands the reply box over the list's foot whenever the box drawn under its row would end below the foot. It never checks the row being answered, so when that row ends less than a box's height above the foot, the box is drawn over it.
- The last card is the usual case: Reply on it with the list anywhere short of its very end. With the reader 40 px short of the end, the box covered 68 of the row's 86-107 px; at 120 px short it covered the whole row. With one more card below, the target sits a card higher and the docked box covers that card instead.
- The promoted (also-in-the-channel) reply only makes the box taller (its destination line and Post to main): 254 px against 228. It covers more, but promotion is not the trigger.
- Second defect, same path: Reply opened at the very end hides the column's own box after the row was held. The browser's scroll clamp then moves every row down 42-70 px.
- Third: the foot keeper chased the room the box keeps under its row (the rows moved 76 px when a card mid-screen was answered with the list at its end).

# Design

- At the moment Reply opens (the reader's own act), ONE move: if the docked box would cover the answered row, the list goes up by exactly the overlap, so the row's foot sits just above the box. A row taller than the room keeps its top at the head instead.
- Nothing moves where the box covers nothing (#2063 stands).
- The row is held again after the column's own box is hidden.
- The foot keeper does not follow while a reply box stands open in the stream.
- After the open: no drift, and an arrival while the box is open moves nothing.

# Acceptance Criteria

- A walk on phone and desk, light and dark: Reply on the last card (plain and promoted; the list at its end and 40 and 120 px short of it) leaves the row wholly above the box.
- Reply on the second-to-last card: the same, with the card below on screen or offered by Newest.
- A card with room under it does not move (0.5 px).
- The phone keyboard really up after Reply never puts the box over the row.
- 30 frames of no drift after the open; an arrival while the box is open moves nothing.
- Negative control: main's page fails the cover check.
- Owner-UI evidence: phone and desk, light and dark, keyboard open and closed, Discuss open, with BUILD.txt. Name the landed commit for the OSS redeploy.

# Notes

Filed at the coordinator's request (owner-UI fast track), issue file only. Worked in its own worktree on current main, not stacked on #2385.
