---
title: 'jump-to-unread-chip: a chip to jump to the last unread message above the view'
status: open
priority: 1
issue_type: task
created_at: 2026-11-09T22:31:16.852141660+00:00
updated_at: 2026-11-09T22:31:16.852141660+00:00
---

# Description

Owner reply 279 (2026-11-09): "When I'm scrolled to the bottom I
have the little bookmark chip to jump back up to the bookmark
however when everything on the screen is already read ... I'd like
to have another little chip appear which is similar with an upward
arrow to jump to the last unread message. That lets me jump back
to where I was reading when I'm catching up reverse chronological.
I could always switch to the fold red mode. But I don't always want
to do that."

So beside the existing bookmark chip: a floating chip in the same
style with an up arrow (for example "Unread" with the arrow), shown
when everything on screen is already read and unread messages sit
above the view. Tapping it goes to the nearest unread message above
(the newest unread one older than what is on screen), placed with
its top in view like the bookmark jump. It hides when nothing above
is unread, and it works without the fold-read view mode.

# Design

One more floating chip beside the bookmark chip in the channel's
chip row, drawn and worded as its sibling. Its show condition is a
view question: every message currently on screen is read, and at
least one unread message is loaded above the view. Its act is a
jump to the newest such unread message through the existing
scroll-into-view helpers (no new scroll offset writers), and the
jump turns following-the-newest off first, as the #1144
coordination requires; mouse movement or hover never scrolls. A
keyboard shortcut is optional and, if added, consistent with the
shell keymap and its help.

Chip, channel view and scroll-keeper files are shared with other
lanes; the exact files and functions will be cleared with the
coordinator before editing.

# Acceptance Criteria

- The chip shows only when all on-screen messages are read and
  unread messages are above; it hides again when the view changes
  so that either half stops holding, including after the jump.
- With several unread messages above, tapping lands on the newest
  unread one above the view with its top sensibly in view, and
  following-the-newest is off afterwards (later arrivals do not
  pull the reader back down).
- The chip works with the read mode in Show, not only Fold.
- Page tests cover the show and hide conditions, the jump target
  among several unread messages above, and follow mode being off
  after the jump.

# Notes

Owner reply 279, 2026-11-09 5:25 PM ET. Related coordination:
#1144 (a jump up turns follow mode off; hover never scrolls). To be
gated together with #1166 switcher-frame-panel and #1168
read-toggle-two-way under one full gate after the coordinator's
all-clear.
