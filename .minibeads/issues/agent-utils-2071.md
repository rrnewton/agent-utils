---
title: 'vibe-talk-thread-picker-grid-toggle: thread picker: fixed views as a 2x2 grid on phone, toolbar button toggles, explicit close'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-21T13:55:00-05:00
updated_at: 2026-11-21T14:05:00-05:00
---

# Description

The owner likes the thread list overhaul but: "I don't like the haphazard line wrapping with the two rows of buttons for the four buttons for the fixed views ... they should be displayed as a 2x2 grid filling that space. I want maximum clarity and clickability on mobile" and "If clicking the thread button on the toolbar at the bottom of the screen pops it open. Clicking it again should certainly close it. We could also have an x button for it explicitly." The owner also reported unresponsiveness of that interface (owner 301-302, 2026-11-21).

# Design

Phone: the four fixed views (Main, All, Thread list, Transcripts) as an equal-width 2x2 grid filling the popup's width, each cell at least 48px tall; an odd fifth option (Two columns, where it fits) takes the full last row rather than half of it. Desktop (min-width 900px and a fine pointer): one equal row when all fit, else the same 2x2. The toolbar thread button toggles the popup: a second tap closes it, and the outside-tap handler ignores the button itself so the same tap cannot reopen it. An explicit close (X) button on the popup, `aria-label="Close thread picker"`, focusable, at least 44px, returns focus to the thread button; Escape closes too and returns focus; an outside tap still closes.

Responsiveness, measured at 6x CPU throttle at 412px before the fix: the popup painted in one frame, but opening burned 53-99ms long tasks rebuilding every row, and a redraw landed on every render while it stood. Local to the picker, a redraw whose drawn values are unchanged now skips the DOM rebuild, and the toggle-close path draws nothing. The remaining first-open cost is deriving the thread index from the channel's messages, which is page-wide work for the page-freeze lane (#2066 vibe-talk-page-freeze) to weigh.

# Acceptance Criteria

- Grid and toggle behaviour covered by browser checks: four equal cells, two per row at phone width, one equal row on a desk, 48px minimum height; toggle open then closed with no reopen after frames settle; X named, focusable, closing with focus returned; Escape closes.
- Screenshots phone (412) and desktop (1280), light and dark, popup open and closed, plus a toggle sequence.

# Notes

Owner requests 301 and 302 (2026-11-21 1:49 and 1:51 PM ET), fast-track UI. Perf findings for the broader freeze go to #2066 vibe-talk-page-freeze.

Resolution: landed and deployed; the `.thread-picker-group` sticky heading it added is a named exception in the `#226` containment scan (the picker pane is its own scroller outside `#scroll-area`, like Discuss and Labels), keeping the heading whole at open.
