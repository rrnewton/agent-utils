---
title: 'vibe-talk-search-late-hits-shift-anchor: late server hits inserted above the reader hold the anchor in place'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- search
- ui
created_at: 2026-11-19T04:48:00-05:00
updated_at: 2026-11-19T09:10:00-05:00
closed_at: 2026-11-19T09:10:00-05:00
---

# Description

[watermelon-full-preview] Follow-up to #1894 vibe-talk-search-extra-hits-off-by-default and #1880 vibe-talk-no-programmatic-scroll-after-user-scroll (owner scroll-jump threads 202, 204, 206, 207). Finding while proving #1894's zero-scroll half: when late search rounds land (a gated second server page via resume, or a "Search further back" round that lands after the reader scrolled), the inserted server-hit nodes (`placeServerHits` in `vibe-talk/web/src/view/channel/olderwalks.ts`) shift the reader's anchor row down with `scrollTop` unchanged: +105.9 px (gated page) and +286.9/+287.9 px (grow round), identically on 597ba683 and 747cd7a3. No code path routes these insertions through `placeFilterAnchor` or #1880's anchor hold, so nothing compensates. That is exactly the jump class the owner hates.

# Design

- When server hits (or any late rows from `placeServerHits`) are inserted above the reader's anchor after the reader moved, hold the anchor's on-screen place: measure the anchor's `rect.top` before insertion and restore it after, reusing the #1880 hold mechanism (`ScrollKeeping.preservingScroll` via the existing `preservingScroll` dependency of `mountOlderWalks`); do not fork it.
- Insertions below the anchor must not move anything (the hold's delta is zero there).
- No compensation when the reader is pinned at the newest end (the hold's pinned path follows the newest line as before).
- Evidence reuses the late-round stub overlay: BEFORE (main) shows anchor `rect.top` +105.9 / +287 px; AFTER shows delta about 0 for both late paths, phone and desktop, light and dark. Report `rect.top`, not `scrollTop`.
- Add a cheap browser assertion only if fast and condition-waited (rule 346).

# Acceptance Criteria

- Gated second server page landing after the reader scrolled: anchor row `rect.top` delta about 0 (was +105.9 px), phone and desktop.
- "Search further back" grow round landing after the reader scrolled: anchor row `rect.top` delta about 0 (was +286.9/+287.9 px), phone and desktop.
- Insertions below the anchor move nothing; a reader pinned at the newest end gets no anchor compensation.
- Light and dark evidence; existing search/server-hits behaviour otherwise unchanged.
- Dynamic exercise through the real page/server fixture before landing, with `rect.top` numbers and screenshots in the handoff; owner-priority gate green or waived steps named.

# Notes

Lane mu-hitsanchor on branch mu-hitsanchor/1901. Coordinates with #1894 (mu-labelsdrop, whose late-round stub overlay reproduces this) and #1880's scroll owner; reuse its hold, never a second input tracker.

## Resolution

[watermelon-full-preview] Landed on github/main as de9931f9 (e9d687c3 + de9931f9, replayed from gated defe5e32 on f404b565). `renderServerSearch` in `vibe-talk/web/src/view/channel/olderwalks.ts` now places server hits inside the #1880 scroll owner's existing `preservingScroll` hold, and re-marks the placed cards after it (the hold's scroll pass can unmark a card that holds no loaded search text; caught by page-3 `#1797`). Evidence, shot from landed main de9931f9 with harness-format BUILD.txt in /tmp/mu-hitsanchor-main (sidecars rect-top-landed.txt and rect-top-prefix.txt): anchor `rect.top` delta 0.1 px on both late paths (gated second page and Search-further-back grow), phone 412x915 and desktop 1280x800, light and dark, against +105.9 / +286.9,+287.9 px on pre-fix e4a54be6. Gate: full owner gate 177 passed / 1 failed on defe5e32, the sole red `reload-place-browser` (passes solo on both defe5e32 and pristine f404b565; the known race under load), whose one exclusive rerun passed; land-main recorded that flake waiver. Earlier gate on the same change (fa17297e) was 177/1 with sole red `thread-reader-follow-browser`, since disabled on main by #1903 f404b565.
