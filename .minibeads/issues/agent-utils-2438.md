---
title: 'vibe-talk-composer-focus-keeps-reading-row: focusing the channel or thread composer must not take the reader far from the row they were reading'
status: open
priority: 1
issue_type: bug
assignee: cl-loading
labels:
- vibe-talk
- owner-ui
- scroll
created_at: 2026-11-27T22:25:00-05:00
updated_at: 2026-11-27T22:25:00-05:00
---

# Description

Found by the advisory design reviews of #2385 S-t1 and #2426 R2 (rule R12, owner rule 370: nothing moves the row being read except the reader's own act, and that act only as far as it needs to). Focusing the composer of an open thread, or of the channel list, scrolls the list to its foot. A reader who was reading mid-list, or the head of a thread whose replies are still loading, loses their place.

What the reviews measured (local fixtures, phone 412x915 and desk 1280x800):

- On main, focusing the thread composer moved the reading row from 370.67 px to -45.33 px on a phone, off the top of the screen.
- With #2385 S-t1's priced boxes the same act moved it to -235.33 px, because the priced boxes are taller than the count's flat price.
- On the desk, after that focus had put the reader at the foot, a reply fill moved the reading row 448 px on main and 258 px with S-t1.

Focus is the reader's own act. But the act is "I want to type", not "take me to the newest message". The composer already fits itself above the keyboard (#2277), so it needs no scroll of the list.

# Design

- Focusing a composer keeps the row being read where it stands. The composer comes into view by the keyboard fit and its own foot placement (#2277), never by a scroll of the list to its foot.
- Only when the composer itself would be off-screen does the list move, and then only far enough to show it. The anchor is the row being read, through the one scroll owner.
- The arrival rule stays as it is (#2230): no arrival follows the foot while the reader types mid-list.
- Find which writer scrolls to the foot on focus (the composer keep, the foot keeper or the focus handler) and make it stand down when the reader is mid-list. The scroll trace (#1144) names the writer.

# Acceptance Criteria

- A browser walk at phone 412x915 (keyboard open by viewport shrink, verified) and desk 1280x800, in a thread mid-fill and in the channel list mid-history: focusing the composer moves the reading row at most 1.0 px on every frame (whole-pixel rule), while the composer is visible and fitted above the keyboard. The walk fails on main.
- At the foot, focusing the composer keeps the reader at the foot: no regression of #2292 or #2277, and their checks stay green.
- The owner-UI matrix: light and dark, keyboard open and closed, phone and desk, with Discuss open.

# Notes

Filed by cl-loading for the #2426 loading program, which reserved this number (coordinator, 2026-11-27). To be fixed before R2 (islands) or together with it. Related: #2385, #2426, #2230, #2277, #2292.
