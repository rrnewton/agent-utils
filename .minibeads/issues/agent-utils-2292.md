---
title: 'vibe-talk-windowed-composer-foot-fight: with windowed lists on, arrivals and the windowed renderer never move the composer or leave a mid-list jump blank'
status: closed
priority: 0
issue_type: bug
assignee: cl-windowfoot
labels:
- vibe-talk
- owner-ui
- scroll-stability
created_at: 2026-11-26T09:45:00-05:00
updated_at: 2026-11-26T19:50:00-05:00
closed_at: 2026-11-26T19:50:00-05:00
---

# Description

Windowed lists (`#1846`: render only the viewport plus a prefetch margin; device setting `vibe-talk.voice.windowed-lists=1`; default still OFF everywhere, see `windowedConfig` in `vibe-talk/web/src/core/settings/windowed.ts`) cut the cost of what remains after `#2277` about 12x (rows mounted 746 -> 60; a `:has()` toggle 202-220 ms -> 17 ms, measured by the cl-freeze lane on `#2277`'s tree; evidence and script under `/tmp/cl-freeze/evidence-final/windowed/`). The owner will be asked about turning it on by default only after this issue's evidence. With windowing ON, two things break:

- (c)1 Typing at the foot with the keyboard up while 4 messages arrive moves the COMPOSER 45.9 px over 133 frames (0.38 px with windowing off). Several writers move the list against each other: `renderWindowed`'s spacer nudges (-8, +2471, +180), `settleAfterRead` (`toPosition` +8 / -2509, `nudge` -8 / +2471) and `keepComposerInView` (+-135, +28, +115). The windowed renderer's spacer corrections are not composer-aware, and `settleAfterRead`'s foot is 8 px past the composer's fitted foot.
- (c)2 A programmatic jump into the middle of the list (`scrollTop` = half the height) lands on an unmounted spacer: no row on screen for about 0.1-0.2 s at 4x CPU throttle until the slice mounts.

Also NOT yet exercised with windowing on: search hits, jump-to-message, and Back (cross-view Back restore). Exercise them and list anything that breaks.

This is owner-UI fast-track work (owner rule 370, owner request 381 on lag).

# Design

Cause (mu-windowfoot, 2026-11-26, on main after `#2277` landed as fbe206a9):

- (c)1: one arrival render runs three scroll writers in sequence with three different targets. `renderWindowed` (`readcore.ts`) holds a ROW anchor with `deps.nudge` spacer corrections; `settleAfterRead` (`readcore.ts`) restores `previousTop` with `deps.toPosition` and then nudges to the held row, its foot 8 px past the composer's fitted foot (`composerFoot` in `keyboard.ts`, the `#2277` seam); `keepComposerInView` (`scrollkeep.ts`) then writes the composer back (+-135, +28, +115 in the trace). The writes partially cancel frame by frame, so the composer wanders 45.9 px over 133 frames. With windowing off only `keepComposerInView` writes, and the spread is 0.38 px.
- (c)2: the windowed scroll listener in `renderWindowed` defers its re-slice to `requestAnimationFrame` behind a queued flag, and the slice is priced from estimated heights. A programmatic jump sets `scrollTop` into a spacer region; until the deferred re-slice runs, no mounted row intersects the visible band, so the screen is blank spacer for about 0.16 s at 4x throttle.

Fix plan (one owner of the foot; reuse `#2277`'s `composerFoot` seam and the scroll owner in `scrollkeep.ts`, never a parallel keeper; only constant-time work in scroll handlers):

- When the composer is held (`composerBefore` non-null), the windowed renderer's anchor nudge and `settleAfterRead`'s position restore stand down for that render: the scroll owner's composer hold is the one writer of the foot, and its target is the fitted foot.
- `settleAfterRead`'s held-row restore measures against the fitted foot when a composer is held, so its writes agree with `keepComposerInView` instead of landing 8 px past it.
- A scroll event whose visible band intersects no mounted row (a jump onto a spacer) re-slices synchronously in the same task, checking only the first and last mounted rows; the normal near-edge case keeps the deferred hysteresis path.
- Search hits, jump-to-message and Back restore are exercised windowed; any break found is listed in Notes and fixed through the same seams (pins and the scroll owner), not with new keepers.

A counting/trace check for the composer-foot case joins the gate (no wall time; under about 30 CPU s; classified in `validation/components.json`).

# Acceptance Criteria

- Composer movement <= 0.5 px (or <= 1.0 px on whole pixels) while typing at the foot with arrivals, windowing on and off, phone and desktop, keyboard open and closed, light and dark.
- No blank frame on a mid-list jump with windowing on.
- The rule 370 traces (arrivals mid-list and at newest, scrolling past the window edge, own sends) still at 0.0 px.
- Search hits, jump-to-message and Back working with windowing on; anything that breaks is listed and fixed or filed.
- The cl-freeze windowed walk re-run on the fixed tree, Off vs On, reporting the same numbers (rows mounted, toggle cost, composer spread, jump blank time).
- Screenshot matrix with windowing on, `BUILD.txt` naming the exact tree; dynamic exercise on a local test instance through the real server.

# Notes

- Stacking: scroll code is shared. Landing order: `#2277` (landed fbe206a9), the `#2276` follow-up spans, `#2283`, `#2258`, `#2291`, then this issue. Start on current github/main and rebase onto each landed SHA the lead sends.
- Evidence before the fix: `/tmp/cl-freeze/evidence-final/windowed/` (cl-freeze lane, read-only).

## Resolution

Landed on main as a0c2ec41 on 2026-11-26 at about 7:45 PM ET (gated as c81760c6: owner gate red only on `vibe-talk.test.page-3`, a CPU budget timeout with 0 failing tests, the slice budget tracked in `#2320`; its exclusive rerun passed; liaison approved the evidence).

- One writer while the reader types: the scroll owner offers its composer hold (`heldComposerTop`, `keepComposer`) to the writers outside its holds. `renderWindowed`, `settleAfterRead` and `appendChannelRow` keep the held composer instead of a row (no anchor nudge, no foot lock, no row restore while it holds the view); the jump to the newest is still decided.
- Found in scope and fixed: at the desk, windowing on or off, the composer dipped 8.7-9.1 px for a frame on every arrival (the redraw's hold measured it after the row was appended; its edge correction and the list's foot keeper then disagreed by the list padding).
- (c)2: measured at paint time, main painted no blank frame on programmatic jumps (the earlier 0.16 s was polling latency behind the re-slice). The windowed scroll listener now re-slices in the same task when the band stands wholly above or below the mounted rows (two rows measured), so a row is in the band as the jump's scroll is answered (main 0/5, after 5/5).
- Results, whole-pixel bar (scroll positions land on whole CSS pixels): composer max step 0.09 px phone / 0.69 px desk, spread <= 0.69 px, in all 12 cells (windowing on/off x light/dark x phone keyboard open / closed / desk); the cl-freeze walk's windowed composer spread 45.89 -> 0.62 px (off 0.38); rule 370 traces 0.0 px; rows mounted 746 -> 60.
- Search hits, jump-to-message, back gesture and cross-view Back (48 of 48 walks, real server) pass with windowing forced on: nothing broke.
- Gate check `tests/windowed_composer_foot_browser.py` (dag 184): every scroll write while typing through arrivals is the composer keep and the composer moves under one pixel, windowing on and off, phone and desk; jumps into a windowed list have a row in the band. Red on main, green after.
- Not changed here: the history above the composer still moves by each arrival's height while the reader types; that is `#2315 vibe-talk-kb-open-arrival-moves-history-94px`, waiting on the owner's choice.

