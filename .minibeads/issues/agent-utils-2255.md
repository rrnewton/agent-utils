---
title: 'vibe-talk-phone-keyboard-composer-between-chips-discuss: on phone with the keyboard open, the composer row shows between the search chips and Discuss'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-25T14:53:00-05:00
updated_at: 2026-11-25T14:53:00-05:00
---

# Description

Found while shooting the #2250 vibe-talk-all-view-search-both-buckets-default evidence on main 00da5a15 (page build 812cad22), phone 412x915, light and dark: with a search open in All, the Discuss half-pane open and the on-screen keyboard open over the focused search field (viewport really shrunk by 336 px), the band between the search chip row and the Discuss pane shows the channel composer's controls ("Send raw message", Cancel, Send) and the bottom edge of the composer box, not search results. The results the reader just searched for are out of sight above that band.

# Design

- Find why the shrunk list lands at its foot. Candidates: the keyboard shrink follows the newest end or the composer as the focused-field scroll target, though the focus is in the search field at the top; or the composer is laid out inside the list's scroller, so a scroll to the foot shows it.
- With the search field focused, the visible band shows the head of the results: the first result under the chips, as with the keyboard closed. The composer stays at the foot, out of the band, unless the reader scrolls there.
- No movement of a row the reader is looking at when the keyboard opens or closes beyond what the viewport change itself forces (#2230).

# Acceptance Criteria

- A browser check at phone 412x915, light and dark, Discuss open, search open with results: with the keyboard really open (visualViewport height equals the shrunk height) and the search field focused, the first result card is visible below the chip row, and no composer control is visible between the chips and Discuss.
- Keyboard closed again: the same first result is still in view.
- Shots before and after (phone, keyboard open, Discuss open, light and dark) plus the standard matrix, each folder with its BUILD.txt.

# Notes

Owner-UI (not fast-track). Filed by cl-searchview at the coordinator's request, from the #2250 evidence review. Related: #2254 vibe-talk-search-chips-cover-first-card, #1968 vibe-talk-search-close-x (keyboard-open search restore), #1899 vibe-talk-search-bucket-chips-pinned.
