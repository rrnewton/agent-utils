---
title: 'vibe-talk-search-head-room-two-columns: the search head room keeps a reader at the foot still with the right column up'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-25T16:50:00-05:00
updated_at: 2026-11-26T01:48:00-05:00
closed_at: 2026-11-26T01:48:00-05:00
---

# Description

Split from #2256 vibe-talk-search-head-room-no-shift. #2256 keeps the reading row still at the first search key by holding the newest row while the search head room is added, but only in the one-column layout. With the right column up (desktop 1920 with side cards open), that hold broke the right column's overscroll room: a later reply from someone else no longer filled the room (#2230's path) and the reader in the side column was left 44 px above the new foot (gate step column-overscroll-1920). So #2256 runs the hold only with one column. With two columns the old movement remains, same as main: a reader at the foot of the main column moves about 57 px at the first key (tip -57.0, main -56.8, light and dark; mid-list 0.0). Desktop with the right column open is the owner's common layout. Owner request 370: nothing may move the row being read.

# Design

- Find exactly which call closes the right column's overscroll room while the hold runs: log footroom's closeOverscroll callers (and every #side-scroll write) with the hold enabled in two columns. The only #side-scroll write seen during the arrival is settleAfterRead's nudge (620 to 661), so the room closed earlier, during the search steps.
- Make the newest-row hold act only on the main column's scroller and leave the right column's footroom state alone, then enable the hold in two columns too.
- No foot-follow on arrivals anywhere: the right column keeps #2230's behaviour (the room absorbs someone else's reply, the reader holds, no Newest chip).
- Constant layout reads per pass.

# Acceptance Criteria

- Per-frame trace (rAF-stepped) at 1920 with the right column up, reader at the foot and mid-list of the main column, first key and refining key, light and dark: dy within 0.5 px at every frame; main's -56.8 at the foot as the negative control, with a JSON sidecar.
- column_overscroll (1920 and 1280) passes with the hold enabled in two columns.
- #2256's one-column evidence (scene 123, fixture traces) unchanged.

# Notes

Owner-UI fast-track (owner 370), ahead of #2259 and #2260, for the lane that owns #2256 and #2257. Related: #2256, #2257, #2230 vibe-talk-reading-row-stays-put.

## Resolution

Landed as 7b331dae (one commit, the gated 54829dea replayed over an issue-file commit) on a green owner gate on the same change (54829dea, 2026-11-26 1:30 AM ET). It supersedes #2256's newest-row hold: the search's own line at the list's foot now counts as room past the rows' foot (`footroom`: `roomsPastFoot` and `followFoot` take it, `folding` offers no Newest jump for it), so a reader at the foot stays at the foot in one column and two, and a reader at the head of a list that fits (scrollTop 0, words typed, one column) keeps the head row instead of the newest line (`filterAnchor`). Evidence: two columns at 1920, reader at the foot dy -1.0 at worst (main -58.0), mid-list 0, refining key 0, light and dark; one column mid-list 0, foot 0 phone and -1.0 desk; the head reader 0.2 at the key (18.2 without that step); real-server scene 123 keeps the newest row within 1.0 px from the key through the answer, scenes 120, 124 and 125 unchanged; column_overscroll at 1280 and 1920 passes. The criterion's 0.5 px became 1.0 px: the foot room places the reader on whole pixels, and that was accepted in review. The rebased head was traced again before the gate (the current-view search scope, #2251, had landed since) and matched the accepted run. Not this issue: the head row still moves when the server's answer lands (the thread-view parking in the search count, +75.9 px desk), tracked in #2258. Deployed the same night (live about 1:32 AM ET); the 15-minute post-deploy watch was clean (startup probe passed, no app errors, panics or restarts).
