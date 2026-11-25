---
title: 'vibe-talk-search-fast-pass-holds-reader: the search fast pass never moves the reading row when it hides rows above it'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-25T15:16:00-05:00
updated_at: 2026-11-25T18:31:00-05:00
closed_at: 2026-11-25T18:31:00-05:00
---

# Description

Found while tracing #2256 vibe-talk-search-head-room-no-shift, and on main: in a long channel (over 200 rows, where `#1913`'s typing-fast pass runs inside the input event), a reader scrolled into the middle of the list who types the first key of a search ('routine', matching the row being read) sees that row jump -1334.8 px at the key, at 412x915 and 1280x800 alike. The fast pass hides the non-matching rows above the reader (16 here) without the scroll owner's hold, and the settled pass that lands after the pause does not put the row back. Checked with real wheel input and a 3 s rest before the key, so the scroll owner had noted the reader's place. Owner request 370: nothing may move the row being read.

# Design

- The fast pass keeps the row being read by its top when its visibility writes change what stands above it: the scroll owner's hold around the pass (or a measured compensation of the height it took away or added above the anchor), on every key, not only the first.
- Bounded by the typing-fast budget: a constant number of layout reads per key (the anchor's neighbourhood, never one per row), inside `search_typing_fast_browser.py`'s bound; no full-list layout read and no per-row measurement.
- The settled pass's place-keeping then has nothing to undo, and its result is unchanged.
- At the very top of the list there is nothing above to hide, and a reader at the foot keeps the foot as today.

# Acceptance Criteria

- A per-frame trace (rAF-stepped) of a reading row mid-list across the first key and a refining key, on the 800-plus-row fixture, phone 412x915 and desktop 1280x800, light and dark: dy 0 at every frame; main's -1334.8 px as the negative control, with a JSON sidecar.
- A page test: the fast pass hiding rows above a mid-list reader leaves the row's top unchanged, with the per-key layout-read bound still holding.
- `search-typing-fast` stays green and within its per-key bound.

# Notes

Owner-UI (owner 370). Split from #2256 by the coordinator; stacks on #2256. Related: #1913 vibe-talk-search-typing-lag (the fast pass), #243 filter-keeps-selection, #1901 vibe-talk-search-late-hits-shift-anchor, #2230 vibe-talk-reading-row-stays-put.

## Resolution

Landed as 5850cf36 (three commits: the fast pass keeps the row being read when it hides rows above it, takes that row only when it writes, and screenshot scenes 124 and 125 in a long channel) on a green owner gate on the same change (00e70be2, 2026-11-25 6:09 PM ET). Evidence: on the real server with 250 seeded messages, scene 124 (the row still matches) stays within 0.8 px every frame from the key through the answer and scene 125 (the row hidden, the next standing row takes its place) within 0.5 px, against main's -3427/-4715 and -376/-528 px; a pass that changes nothing reads no layout; 20 interleaved typing-fast pairs pass on both. The held row can still end under the chip panel when it sat just under the search bar: that is #2254. Deployed the same evening; the 15-minute post-deploy watch was clean.
