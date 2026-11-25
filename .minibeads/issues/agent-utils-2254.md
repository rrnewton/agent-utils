---
title: 'vibe-talk-search-chips-cover-first-card: on desk the search chip row covers the first card''s top line'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-25T14:52:00-05:00
updated_at: 2026-11-25T14:52:00-05:00
---

# Description

Found while shooting the #2250 vibe-talk-all-view-search-both-buckets-default evidence on main 00da5a15 (page build 812cad22), desktop 1280x800, light and dark: in All, with the Discuss half-pane open, a search whose server answer places older hits ("5 found (2 loaded, 3 older)") lands with the oldest placed card's top line (author, time and first words) under the floating search bar and its bucket chip row. Only the rest of the card shows below the chips. The same search with Discuss closed and the list scrolled to its head lays the first card out below the chips.

# Design

- Find which path leaves the first result under the panel in this state. Candidates: the server's placed cards are inserted above the reader's anchor and the scroll hold keeps the anchor rather than the head reservation's margin; or Discuss's half-pane changes the list's height, so the hold measures against the wrong top.
- The rule to hold: a result card never starts under the search panel. Either the head reservation (`--bucket-head`) covers it, or the landing parks the first result below the panel through the scroll owner, as a thread view already does (#2200).
- No row the reader is looking at may move when the chips stand or retire (#2230, #2249). Fix the placement, not the reader's place.

# Acceptance Criteria

- A page test: in All, Discuss open, a search whose answer places older hits above the loaded ones leaves the first shown result's top at or below the chip panel's bottom.
- A per-frame trace on desktop 1280x800, light and dark, with Discuss open: the first card's rect.top is at or below the panel's bottom once the answer stands, and the reader's row does not move. The +N px on main is the negative control; JSON sidecar.
- Shots before and after (desktop, Discuss open, light and dark) plus the standard matrix, each folder with its BUILD.txt.

# Notes

Owner-UI (not fast-track). Filed by cl-searchview at the coordinator's request, from the #2250 evidence review. Related: #2200 vibe-talk-search-tab-counts-match-cards (head reservation, thread-view landing), #1899 vibe-talk-search-bucket-chips-pinned, #2249 vibe-talk-search-signpost-hidden-while-reserved, #2255 vibe-talk-phone-keyboard-composer-between-chips-discuss.
