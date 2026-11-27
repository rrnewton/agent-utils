---
title: 'vibe-talk-search-landed-reader-follows-foot: a reader the search entry took to the newest hits follows the foot as the search line and chips change height'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T22:20:00-05:00
updated_at: 2026-11-27T02:12:00-05:00
closed_at: 2026-11-27T02:12:00-05:00
---

# Description

Found while tracing #2260 vibe-talk-search-barely-overflowing-list-clamp on the real throwaway server (the screenshot harness channel, phone 412x915). A reader who is NOT at the head (scrollTop 428 in a list overflowing by about 1860 px) types the first key of a search whose results leave the list a few pixels taller than the screen. With no hit on the screen they typed at, the entry landing takes them to the newest hits (#1734), as intended. After that the reader counts as following the foot: as the search's own line and the chips change height in the next frames, the list follows the foot and the browser clamps it, so the row being read moves 51 -> 20 -> 5 px over about ten frames (reading row worst 46 px). Owner request 370: nothing may move the row being read. #2260 fixed the same shape for a reader at the head; this is the landed reader, which #2260 leaves to #1734 on purpose.

# Design

- The entry landing places the reader once, on the list as it will stand after the search's own height changes (its line at the foot, the chips' room), or holds the landed row by its top instead of following the foot afterwards, so no later keeper or restore moves it.
- #1734's rule stays: a reader with no hit on the screen they typed at still lands at the newest hits.

# Acceptance Criteria

- A per-frame trace on the real throwaway server (phone 412x915 and desktop 1280x800, light and dark) of a search's first key from a reader mid-list with no hit on screen, results a few pixels taller than the screen: after the landing frame, the landed row keeps its top (<= 0.5 px) through the settled answer; main's 51 -> 20 -> 5 px as the negative control, with a JSON sidecar.
- A page test for the landing on a list a few pixels taller than the screen.
- #1734's page test and search_scroll_browser.py unchanged.

# Notes

Owner-UI (owner 370). Filed at the coordinator's request from the #2260 traces (issue file only). Related: #2260, #1734 vibe-talk-search-view-scroll-default, #2262 vibe-talk-search-head-room-two-columns, #2230 vibe-talk-reading-row-stays-put.

Correction of the premise above (2026-11-27, real throwaway server, scroll writes hooked with their stacks): for a reader who has a hit on the screen they typed at, it is not #1734's landing. The key's filter place-keeper keeps the reader's row (or the nearest shown one) at its offset and asks for a scroll the shortened list cannot make (phone scrollTop 149 of a list that can scroll 51; desk 136 of 39): the browser clamps it to the foot, the next frame the list's foot shrinks and the browser clamps again, and a restore with a pinned mark then follows the foot (reading row 31 + 15 px phone, 30 + 9 px desk). With no hit on that screen, #1734's landing does take the reader to the newest hits, and the restores then follow the foot as the search's line and chips change height (15 px on a desk with Discuss open).

Fix: a search's placement that the shortened list cannot reach holds the row by its top with the room the list lacks (`holdRowAt`, as a tapped row is held) instead of a nudge the browser clamps; and the entry landing clears that held room, goes to the newest hits as #1734 has it, then holds the row it landed on (the list's head row where the list now fits, else the newest row) as a filter hold, so no restore or keeper follows the foot from it.

## Resolution

[opus 5.5] Landed in 5b88cf48 (2026-11-27 2:09 AM ET, liaison LAND NOW; owner gate fully green on the gated head 6c47b9da; landed over a disjoint app/main.js change of #2269 with the replayed tree checked: tests/js, the search and filter browser checks with search_scroll, and the page build). A search's placement the shortened list cannot reach now holds the row by its top with the room the list lacks, 8 px clear of the floating search bar where the nearest hit takes a hidden row's place; the #1734 entry landing clears that room, goes to the newest hits, and holds the row it landed on. The premise correction is in the notes above.

Evidence (real throwaway server, reader mid-list, first key of a search leaving the list a few pixels taller than the screen, every frame until 60 after the last change): main moved the reading row 46 px on a phone and 39 px on a desk with a hit on the typed-at screen, and 15 px on a desk with Discuss open without one; the fix 0.0 px in every case, light and dark, phone with the keyboard open included. Module test "`#2346 ...`" in tests/js/view/channel.test.mjs passes on the fix and fails on main; #1734, #1968, #2260 tests and search_scroll_browser.py unchanged. Liaison decisions: the about 100 px of room left below the composer is accepted (the hold's room, R12 over R10), and so is the Send row ending 16 px under the dock after the key with Discuss open. The one-off search_unmatched_browser red seen on the way is #2357.

