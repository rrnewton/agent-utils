---
title: 'vibe-talk-search-barely-overflowing-list-clamp: a list that barely overflows follows its foot and clamps back, moving the row being read for a frame'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T15:58:00-05:00
updated_at: 2026-11-25T15:58:00-05:00
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
