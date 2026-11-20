---
title: 'vibe-talk-search-close-x: a prominent X closes the search area on phone and desktop'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
- search
- ui
created_at: 2026-11-20T11:08:00-05:00
updated_at: 2026-11-20T11:08:00-05:00
---

# Description

[watermelon-full-preview] Owner request (230, 11:08 AM ET 2026-11-20, verbatim): "I'm on mobile and using the search interface and I'm not very used to the button placement yet. I think both on mobile and on desktop we should have a prominent X for closing the search area, probably on the right side of whatever the user interface is. Clicking the magnifying button makes the search go away as well, but that's not super obvious from the UI."

The magnifier toggle already closes search (clearing the query and its filters and restoring the previous view and scroll through the place-keeping close path), and Escape in the field does the same. Neither is discoverable as a close control. Add a prominent X on the right side of the open search area, on phone and desktop.

# Design

- One button, id `search-close`, inside `#search-float` after the magnifier, so it is the rightmost control of the open bar. Accessible name "Close search", keyboard-focusable, at least a 44px touch target.
- Clicking it calls the same close path as the magnifier toggle and Escape (`setSearchOpen(false)`, then settling focus as Escape does); no duplicated close logic. It is hidden whenever the search field is hidden, drawn by the one control-bar render that owns the bar's members.
- It must not collide with the search field, chips or bucket controls at 360px and 412px widths, or with the on-screen keyboard open. Escape already closes from the field; no change needed there.
- Tests: extend the page suite (X visible only while open, its label, clicking it closes and restores every row exactly as the magnifier does), extend the filter-chips unit test (the X listener drives the same close), and add a real-Chromium check `search_close_browser.py` (registered as a new dag.d fragment) that opens search, searches, scrolls, clicks the X, and proves the view and scroll are restored at phone and desktop sizes.
- Restore (found by that browser check on devbig030, keyboard open): a typed search whose entry landing moved the reader to the newest hits (no hit in the visible — keyboard-shrunk — band) must give the reader their pre-typing place back on close. `noteSearchScreen` now also captures the filter anchor before a new entry, `landSearchEntry` marks when it takes the reader to the foot, and closing (any of X, magnifier, Escape, through `setSearchOpen(false)`) restores that anchor after the bar folds — only when the entry moved the reader, and never over a message the reader chose in the results, so `#243`'s kept-place rule stands for every other close.

# Acceptance Criteria

- Open search at 360px, 412px and 1280px: the X is visible on the right of the bar, named "Close search", focusable, its target is at least 44px, and it overlaps no field, chip or bucket control.
- Clicking the X closes the search area, clears the query and filters, restores every row, and restores the previous view and scroll, exactly as closing with the magnifier does; Escape in the field still closes.
- With search closed the X is not visible and takes no taps.
- Page unit test and browser check pass; screenshot matrix (light and dark, phone 412 and laptop 1280, keyboard open and closed, Discuss open, search with and without a query) is run by coordb on devbig030 from the landed SHA.

# Notes

Owner-priority, fast-track: deployed on landing. Browsers cannot run on this host: the browser check and screenshot commands are handed to coordb with the exact SHA. Related: #129 message-search, #197 floating-search, #1852 vibe-talk-search-whole-threads-toggle, #1899 vibe-talk-search-bucket-chips-pinned.
