---
title: 'search-view-unstable-while-searching: rows jump on screen as the query changes and as search results arrive'
status: open
priority: 0
issue_type: bug
assignee: cl-searchux
labels:
- vibe-talk
- search
- owner-ui
- scroll
created_at: 2026-11-27T21:45:00-05:00
updated_at: 2026-11-28T03:20:00-05:00
---

# Description

The owner, 2026-11-27 7:09 PM ET: "Second the view is pretty unstable while searching..."

What happens: while the reader types a search, the rows on screen jump. Each key changes which rows stand; the search's deferred marking, the count line and chips, the server's answer and its placed hit cards each redraw the list a moment later. Owner rule 370 (ui-rules R12): the row being read never moves; growth extends below.

Existing work this builds on: `#2346` (landed) holds the row a search's first key lands on by its top; `#2354` (open) tags the search's deferred mark redraw, its full passes and the server hits as cause `"other"`, so they never follow the foot. What is not yet measured is the drift of the reader's anchor row per keystroke after the first and per result arrival (deferred marks, server answer, placed hits, count line and chips), and whether any of those still moves it.

# Design

- Measure first: a per-frame trace of the anchor row (the hit nearest the reading line, else the row the search keeps in place) across a typed query, one key at a time and fast, and across each result arrival, on a phone and a desk. Each frame that moves the anchor gets its writer stack-traced, as `#2354` did.
- Every change the search makes while it stands is laid out around the anchor row by its top: a key that keeps the anchor shown keeps its top; a key that hides it keeps the next shown row nearest the reading line; arrivals extend below, never above the anchor, and never follow the foot. Reuse `#2354`'s cause rule where it applies; do not add a second mechanism.
- No wall-clock budget (owner rule 346): the walk waits on the page's own events.

# Acceptance Criteria

- A local browser walk (fixture server, never the live server): a reader mid-list types a query one key at a time and fast, and the server's answer and its placed hits arrive; the anchor row moves at most 0.5 px per key and per arrival (or 1.0 px where placement is on whole pixels, stated in the evidence), phone (412x915) and desk (1280x800), with a JSON sidecar. The walk fails on main and passes with the fix.
- Owner-UI matrix: desk and phone, light and dark, keyboard open and closed, Discuss open, with before and after screenshots and BUILD.txt; the advisory design review.
- `#2346`, `#2354` (once landed), `#1734`, `#2260` page tests and the search walks (server hits, typing fast, bucket chips, search scroll) unchanged.

# Notes

- Measured (fixture server answering the search, server hits placed as cards; per-frame trace of the row being read, the first row standing below the floating search panel): with the reader near the newest rows, main moved it 40 px then 140 px at a desk and 39 then 111 px on a phone over 300 loaded rows, and 29 then 57 px on a phone over 120 rows; mid-list it held. Writers, stack-traced: (1) over 200 rows the typing's fast pass (`#1913`, `#2257`) nudged the row taking the reader's place into the browser's clamp where the shortened list could not scroll that far, the foot settled a frame later, and the settled pass then measured the newest line the clamp left the reader at and followed the foot; (2) the chips' row shrinking (the jump pill going as the search narrows the list) took its room out of the list's foot under a reader near it; (3) the foot room a hold grows is short by part of a pixel (`scrollHeight` is a whole number), 0.66 px on a phone as server hits were placed, at the foot the browser clamps to: left as it is (a +1 px room for it made the mention jump walk's Load older intermittently not draw under load, found by the owner gate), and the walk allows 1.0 px on such whole-pixel frames.
- Over 200 rows the entry does not land on the newest hits (`#1734`): the fast pass keeps the reader's place (`#2257`), now held through the settled pass. Whether a long list should land as a short one does is a separate question for the liaison.
- Owner UI, fast-track (owner 7:09 PM ET 2026-11-27). Filed at the coordinator's request (issue file only); the same owner message's first half is `#2423`.
- Related: `#2346`, `#2354`, `#2422` search hits vanish after a redraw, `#2425` date filter, `#2426` loading program, `#2230` the reading row stays put.
