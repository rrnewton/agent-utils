---
title: 'bookmark-chip-clear-popover: press and hold the Bookmark chip to clear the bookmark'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T02:14:00-05:00
updated_at: 2026-11-25T02:14:00-05:00
---

# Description

Owner request 369: "Can we add press and hold with a pop op option to clear bookmark on the bookmark chip. I don't know how to clear it on the mobile UI." A long press on the Bookmark chip (beside Newest in the chip stack) opens a small popover with "Clear bookmark". Choosing it uses the existing bookmark-clear path, Undo chip included; no second clear path. A normal tap or click keeps jumping to the bookmark. Desktop gets the same through right-click (contextmenu) and long press. The current phone path (jump with the chip, then tap the X on the bookmark line) leaves the X at 28px, under the 44px target the chips beside it provide.

# Design

Press-and-hold on the chip (threshold matching the row hold, cancelled by movement beyond the swipe-start distance or by scroll, suppressing the following click so it never also jumps, with selection and the browser callout prevented) and contextmenu on desktop open one small popover anchored to the chip, clamped inside the viewport in the manner of the existing chip popovers. Its single item, "Clear bookmark", calls the existing removal entry point so the server write, redraw and Undo chip are the same as the line X, Shift+B and the row menu item. Escape, an outside tap, choosing the item, or the chip hiding closes it; Escape returns focus to the chip. Reuse the existing popover placement helpers; no new clear path.

# Acceptance Criteria

- Long press opens the popover on phone and desktop; a tap still jumps; a scroll starting on the chip does not open it; contextmenu opens it on desktop without the browser menu.
- Clear clears through the existing path and shows Undo; Undo restores the bookmark.
- Escape and outside-tap close the popover; Escape returns focus to the chip; a long press never fires the jump and starts no selection or callout.
- Page tests at phone and desktop for the above; screenshot matrix including the popover open on phone (light and dark, keyboard closed), the state after Clear with the Undo chip, and the chip stack without a bookmark.
- The bookmark line X hit area meets the 44px target the chips provide (glyph unchanged).

# Notes

Owner UI request (fast-track). Prior work: the existing clear path and Undo, the jump-to-unread chip beside it, the row hold gesture, and the existing popover placement helpers. Overlap to clear with the coordinator before editing: the search-tabs lane and the saved-chip lane both touch the chips stylesheet.
