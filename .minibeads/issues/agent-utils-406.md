---
title: 'command-bar-one-row: the desktop command bar stays one row, across the window'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- ui
- desktop
created_at: 2026-10-21T15:47:28.813857185+00:00
updated_at: 2026-10-21T15:47:28.813857185+00:00
---

# Description

[opus 5.5] The owner, 2026-10-21, on a desk: "Right now I'm having a rendering problem on desktop where Thread list view (because of the length of the words 'Thread list') spills onto two rows in the command bar even though there is PLENTY of horizontal space.. an artificial width construction. The other views don't do this but selecting a specific long thread name does it. Let's keep that bar one row that can go up to full width on desktop and size our widgets appropriately."

On a desk (900px and wider, fine pointer) the dock (the gear, the channel and view pickers, the layout button, the reading buttons and the Channel/Voice switch) is held to the reading column. It is the same width as the list above it, which at 1280x800 with the default reading width is about 714px of a 1280px window. When the pickers and the reading buttons did not fit in that column together, the reading buttons wrapped onto a second row. A view label like 'Thread list', or a thread whose name fills the picker's 28 characters, was enough to cross that line, even though half the window beside the column was empty. In two columns the dock already spanned the window (#373 two-column-full-width), so the switch also moved between one-column and two-column views.

Wanted: on a desk the bar is always one row and may use the whole window width, as in two columns. A label too long for the room left ends in an ellipsis instead of wrapping, and the picker's tooltip carries the full thread name. Phone layouts are unchanged.

# Acceptance Criteria

A Chromium check at 1280x800 and 1920x1080: one row with 'Thread list' chosen and with a thread whose name is 120 characters chosen; the 393px phone layout unchanged (negative control). Before and after screenshots.
