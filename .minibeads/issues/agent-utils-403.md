---
title: 'composer-key-global: Ctrl+S (Cmd+S) reaches a message box from every focus and selection state'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- keyboard
- ui
created_at: 2026-10-21T15:31:44.784642920+00:00
updated_at: 2026-10-21T15:31:44.784642920+00:00
---

# Description

[opus 5.5] The owner, 2026-10-21, on a desk in two columns: "I was just trying 'Cmd-s' to select the message entry box... it didn't work. E.g. when a message in the right column is highlighted and focus is elsewhere. But if focus was in the left column it worked. It seemed selective. It should be globally applicable."

#248 composer-key promised Ctrl+S (Cmd+S on a Mac) goes to the message box from anywhere. Measured in Chromium at 1280x800 on main 7045cb1a, the chord was refused to the browser everywhere (no Save page), but in these states the focus did not move and no box took the typing:

- two columns with the right column on its thread cards (no thread open): the focus on a card, a highlighted card, the right column's list, or its Unread only button; and the page body after the right column was the one used last;
- the thread list on its own (Ctrl+3): from a card, the list, or the body.

In two columns with a thread open in the right column the chord did reach a box, the right column's when the focus or the last selection was there and Main's when it was in the left column. A reply open at the foot of one column took the chord from the other column too, so a reader in the right column's thread was sent to the left column's reply.

The rule wanted: the box of the column the focus is in, or (with the focus in neither, as on the bar or the page body) the column holding the selection; where that column has no box (the thread cards), Main's. A reply open at the foot of a column is that column's box. The thread list, like Settings and Help, is left first for the view it was chosen from, because the box is in the column it covers. The chord is heard before any element's own listener, as the search's Ctrl+/ is since #344 search-shortcut, so nothing on the page can keep it from the box, and the browser's Save page never opens.

# Acceptance Criteria

Page tests for each state above, failing before the fix where it reproduced; a negative control (in a text field the chord still moves to the message box and never saves the page); a Chromium walk at 1280x800 in two columns with a thread open pressing the chord from the right-column selection, the left-column selection, a button and the page body; the rule stated in Help.
