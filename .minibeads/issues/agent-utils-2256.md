---
title: 'vibe-talk-search-head-room-no-shift: adding or releasing the search chip panel''s head room never moves the reading row'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-25T15:07:00-05:00
updated_at: 2026-11-25T15:07:00-05:00
---

# Description

Found by the per-frame traces of #2251 vibe-talk-search-current-view-scope, and on main: at a search's first key the list's head room for the pinned chip panel (`#2200`'s `data-bucket-head` reservation, `--bucket-head`) is added without the scroll owner's hold, so the rows under the reader shift down by the room's height: +59.2 px on desktop on a thread with its own hits, where the chips stand while the answer is loading. Owner request 370: nothing may move the row being read ("I want my message to stay on the screen and stable").

# Design

- Add the head room through the scroll owner's hold (`preservingScroll`), in the same frame and mutation that sets the flag, as `#2200` already releases it on settle; or reserve it in a way the browser's own scroll anchoring holds. Either way the row being read keeps its top to the pixel.
- The release keeps its hold and pairs with `#2249`'s rule (the signpost comes back in the same mutation as the release), so neither the add nor the release moves the reading row.
- A reader at the list's foot stays at the foot, with the rows on screen where they were.

# Acceptance Criteria

- A per-frame trace (rAF-stepped) from the key-down frame through the settled answer, for a reading row mid-list and for a reader at the foot, phone 412x915 and desktop 1280x800, light and dark: dy 0 at every frame on the fixed build; main's +59.2 px (desktop) as the negative control, with a JSON sidecar.
- The same across the head room's release (settled with no chip).
- A page test for the add and the release; shots of the moment around the first key, each folder with its BUILD.txt.

# Notes

Owner-UI fast-track (rule 286). Stacked on #2249 vibe-talk-search-signpost-hidden-while-reserved (the same head code). Related: #2200 vibe-talk-search-tab-counts-match-cards (the reservation and its release on settle), #1899 vibe-talk-search-bucket-chips-pinned, #1901 vibe-talk-search-late-hits-shift-anchor (the same hold for late hits), #2230 vibe-talk-reading-row-stays-put.
