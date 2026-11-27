---
title: 'two-column-search-replies-place-left-scroll: a walk in the right column, or the search drawing its marks late, moves the reader''s left column when it sits at its foot'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- vibe-talk
- owner-ui
- scroll
created_at: 2026-11-27T00:23:00-05:00
updated_at: 2026-11-27T02:10:00-05:00
---

# Description

A page defect (owner rule 370: the row being read never moves; growth extends below), found from `vibe-talk.check.two-column-browser` failing twice in an hour on unrelated heads at normal load with the same numbers: `search-replies-place-1280: the cards' walk scrolled the left column`, the left column's `scrollTop` 379 -> 563 (gate `3b18855a` on `8cfd09e0`, 2026-11-26 11:28 PM ET; gate `0733039c` on `dd2ef05e`, 2026-11-27 12:20 AM ET, load1 107 to 151).

The reader scrolls the right column's cards and presses "Search replies before". When the left column sits exactly at its foot, the left column moves one row (184 px): the rows on screen move up by the same 184 px. An instrumented copy of the check recorded it in 4 of 108 runs at four in parallel on main, never alone. The writer, stack-traced and source-mapped: the search field's deferred first marking (`core/search/field.ts` `markSearchNow`, a 120 ms timer armed by typing) lands after the walk's press under contention. Its redraw (`app/main.js`, inside `preservingScroll`) grows the left list, and `restoreScroll`'s pinned branch (`scrollkeep.ts`, `mark.pinned && !mark.userCaused && !actedSince`) follows the foot: `scrollToNewest` -> `followFoot` -> `toPosition` -> `writeTo`. The left column's `overflow-anchor` is `none`, so no browser anchoring is involved. Holding that marking until the walk's press reproduces it every time (main 3/3).

Latent defects sit behind it: the walk's matching rows in the left column, and the `#2269` None-match sentence after the server's answer, are brought up to date on main only as a side effect of that follow. When the server's answer is placed (`renderServerSearch` -> `preservingScroll` -> `restoreScroll` -> `scrollToNewest` -> `setJumpNewest` -> `renderScrollTools` -> `applySearch`), the rows appear. A reader up the list never sees them.

# Design

Causal, not timed (liaison direction, agreed with `cl-signpost3` as one design for `#2346`, `#2351` and `#2354`):

- A `ScrollCause` on the `ScrollMark`: `captureScroll(cause)` and `preservingScroll(mutate, cause)`. In a channel list, a change named `"other"` (growth the reader did not cause there) never follows the foot: the row being read is held by its top, and the jump offers what came below, as an arrival already is since `#2230`. A pinned list still at its foot after such a change is left alone, with no row measured (`#249`). An arrival, the reader's own act, or a change that names no cause follows as before. The call transcript keeps its own following contract (`#431`), as for `#2076`.
- Tagged `"other"` (owner decision via the liaison, items 1 to 3, each a one-line flip at its caller): the search's deferred mark redraw (`main.js`), the search's full passes (`searchrows.ts` x2), `#1901` server hits and the walk's own note and head change (`olderwalks.ts`), summaries (`folding.ts`), the regroup (`channelrows.ts`), the pins (`pins.ts`).
- What the search must redraw is called explicitly where it arrives, not left to the follow's side effect: `serversearch.ts` `foldRepliesPage` runs the search's full pass over the main column after folding the walk's older page, and `olderwalks.ts` `renderServerSearch` runs it once the server's answer is placed (which rows stand, the None-match line's words of `#2269`, the count). Both used to happen only when the newest line was followed (`scrollToNewest` -> `setJumpNewest` -> `renderScrollTools` -> `applySearch`), so on main a reader up the list never saw the walk's left matches and kept a stale `#2269` reach sentence; this fixes both latent defects. The `#243` filter hold and the jump stay where their own semantics release them.

# Acceptance Criteria

- The deterministic reproduction (the search's first marking released just after the walk's press) fails on main and passes on the fix; a per-frame trace of the left column's rows during the walk moves them at most 0.5 px on the fix (main: 184 px).
- `two_column_browser.py` passes 40 runs at four in parallel on the fix and gains a guard: after the walk, every row the left column showed still shows and the walk's matches join them (main: 4 -> 7 rows), failing on a build that holds the foot without the explicit pass.
- A page test covers the cause rule (held for "other", followed for an arrival, the reader's own act, no cause, and in the call transcript), with a negative control.
- The screenshot matrix of the two-column desk (light and dark) with `BUILD.txt` naming the gated tree, and the advisory design review.
- Arrivals, `#1839`, `#1880`, `#1856`, `#2076` and `#1734` unchanged; no wall-clock budget added (owner rule 346).

# Notes

Assigned to `cl-flaketriage` by the coordinator (2026-11-27), first as a test intermittent, then as the page fix (owner-UI class: evidence before any fix). Order: `#2346` lands first, then `#2354`, then `#2351` (`cl-signpost3`) on top, passing `"other"` at its `chipsFollowFoot` caller.

Owner follow-ups, for the morning summary: the fold-read pass (`readcore.ts` `applyReadRuns`) is left untagged, because it is shared with reader acts (Enter opening a read run, the read-mode toggle, thread entry); its background passes should hold per caller. `#249`'s same-search page test now takes the jump before its counted redraw: the search's own marking growth holds the reader's row (item 2), so the jump is offered and its "rows below" count would otherwise be measured on the redraw. Its bound of 4 row reads is unchanged. Today a reader parked at the foot of the channel list does not move on an arrival (0.00 px per frame at 1280x800 and 412x915, light, on main `0f1b1b7e`); the call transcript still follows (`#431`).
