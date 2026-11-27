---
title: 'vibe-talk-search-barely-overflowing-list-clamp: a list that barely overflows follows its foot and clamps back, moving the row being read for a frame'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T15:58:00-05:00
updated_at: 2026-11-26T23:12:00-05:00
closed_at: 2026-11-26T23:12:00-05:00
---

# Description

Pre-existing, same on main. Found while tracing #2256 vibe-talk-search-head-room-no-shift on the real throwaway server (the screenshot harness channel, phone 412x915). With the reader at the head of a channel list that, after the search's first key, overflows the screen by only a few pixels, the reader counts as both at the head and at the newest line: the filter's place-keeper follows the foot (scrollTop 0 to 21) in the key's pass, and on the next frame the list's foot shrinks and the browser clamps it back to 0, so the row being read moves by 21 px for one frame and back. Measured with scene 123's trace (an earlier draft with the word 'recycled' and the reader parked at the head); the base page script of main plus #2249 swapped into the same server shows the same 21 px, so it is not #2256's change. The same shape appeared in the typing-fast fixture's lantern thread at phone size on 'zzqv' (an 11 px follow, back to 0 at the answer). Owner request 370: nothing may move the row being read.

# Design

- A reader who is at the head of a list that fits, or overflows by less than the rest slack, is a head reader for a filter change and for the restores after it: the change keeps the head row's top instead of following the foot.
- Or the follow uses the foot as it will stand (after the pass's own height changes), so no clamp undoes it a frame later.
- The newest-line follow for a list that really overflows stays as it is.

# Acceptance Criteria

- A per-frame trace on the real throwaway server (phone 412x915 and desktop 1280x800, light and dark) of a search's first key with the reader at the head of a list that overflows by under 24 px: dy 0 within 1 px at every frame; main's 21 px as the negative control, with a JSON sidecar.
- A page test with a list that overflows by a few pixels.
- Shots before and after, each folder with its BUILD.txt.

# Notes

Owner-UI (owner 370). Filed at the coordinator's request from the #2256 traces. Related: #2256 vibe-talk-search-head-room-no-shift, #2257 vibe-talk-search-fast-pass-holds-reader, #243 filter-keeps-selection, #2230 vibe-talk-reading-row-stays-put.

Cause (2026-11-26, real throwaway server, reader at the head of the channel list, first key of a search whose results leave the list a few pixels taller than the screen; scroll writes hooked with their stacks): the reader was turned into a newest-line follower. The entry landing (#1734: no hit on the screen the reader typed at, so `scrollToNewest`) let the filter's hold go; then the chips' resize keeper (`chipsFollowFoot` -> `toFoot`) and a later restore (`restoreScroll` with a pinned mark -> `scrollToNewest`) followed the foot as the search's line and chips changed height: 0 -> 20 -> 5 px on a phone. On a desk the key's own place-keeping put the nearest shown row in the hidden head row's place (`nudge` 0 -> 31), which the shortened list could not hold: the browser clamped it to 9 a frame later, and the restore took it to 0.

Fix: a reader at the head with search words typed (`atSearchHead`) gives a head anchor; where the change hides their row, they keep the list's head (`toPosition` 0) instead of a place below it for the nearest row. The entry landing does not take a reader who typed at the head to the newest hits when the newest line already stands within the slack of the screen, so the filter's hold stays and neither the chips' keeper nor a restore follows the foot. Every other reader lands as #1734 has it (its page test and `search_scroll_browser.py` unchanged).

## Resolution

[opus 5.5] Landed in fc355d03 (2026-11-26 11:10 PM ET, liaison LAND NOW, owner gate fully green on the gated head b62861dd). A reader at the head with search words typed now gives a head anchor and keeps the list's head where the change hides their row, and the search entry no longer takes a reader who typed at the head to the newest hits when the newest line already stands within the slack, so the filter's hold stays and neither the chips' keeper nor a restore follows the foot.

Evidence (real throwaway server, reader parked at the head, first key of a fresh search leaving the list a few pixels taller than the screen, every frame until 60 frames after the last change, JSON sidecars): main moved the reading row 20 px on a phone, 31 px on a desk and 15 px on a desk with Discuss open (light and dark); the fix 0.0 px with scrollTop 0 in every frame on phone and desk, light and dark, phone with the keyboard open, and desk with Discuss open (the keyboard-open case did not move on main either). Page test "`#2260 ...`" in tests/js/page/search.mjs passes on the fix and fails on main ("the key's own change moved the head reader to 6"). #1734 unchanged for every reader not at the head and for a head reader of a long list: its page test, #1968's and search_scroll_browser.py pass, with ten more search and filter browser checks. Advisory design review: no finding new against main; the pre-existing ones were left as they are at the coordinator's word. The landed reader who then follows the foot (51 -> 20 -> 5 px) is #2346.
