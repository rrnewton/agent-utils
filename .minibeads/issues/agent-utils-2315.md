---
title: 'vibe-talk-kb-open-arrival-moves-history-94px: on a phone with the keyboard up and the composer focused at the foot, a live arrival moves the history up by the new row''s height'
status: open
priority: 0
issue_type: bug
assignee: cl-windowfoot
labels:
- vibe-talk
- owner-ui
- scroll-stability
created_at: 2026-11-26T17:05:00-05:00
updated_at: 2026-11-26T17:05:00-05:00
---

# Description

Phone (412x915, touch), keyboard open (visual viewport 579 px), channel composer focused, the reader at the foot of the history: a live arrival moves every history row up by -94.0 px in the frame the arrival is drawn, and they stay there. Measured by the cl-arrival lane's arrival walk (kb-open cells, light and dark, per-frame trace of the reading row's top: 8.63 -> -85.37 at the arrival, unchanged through the held re-read and its landing). The same on main before `#2291 vibe-talk-top-anchor-arrival-27px-race` (-102.0 px, which also had the 27 px loading-line race) and on `#2291`'s tree (-94.0 px). With the keyboard closed the same walk holds the reading row at 0.0 px.

Cause, as observed: the composer sits inline at the foot of the list. With the keyboard up it is "held" (`composerHeld`, `#1839`), and the scroll owner keeps it where it stood (`keepComposerInView`, its foot the keyboard's fitted foot, `composerFoot`, `#2277`). The arrival's row is inserted between the last history row and the composer, so holding the composer moves the whole history up by that row's height: the list follows the composer.

Liaison ruling (2026-11-26): NOT intended. Owner rule 370 (`#2230 vibe-talk-reading-row-stays-put`): no auto-follow on arrivals, the row being read never moves (ui-rules R12). Owner 379: "screen jumped a lot while I was trying to type".

Expected: the reading row stays put; the new message appears below it (the Newest chip may show).

# Design

It is the same composer-hold code as `#2292 vibe-talk-windowed-composer-foot-fight` and is fixed in the same lane, as a separate commit if cleaner, through the same seams (the scroll owner's composer hold in `scrollkeep.ts`, `composerFoot`), never a parallel keeper.

Open question for the owner (raised through coordb before any code): with the composer inline at the foot and the keyboard up, the visible band ends at the composer's fitted foot, so a drawn arrival can only go between the last row and the composer. Holding both the reading row and the composer still therefore means the arrival's row is not drawn into the band while the composer holds the foot with the keyboard up; the alternatives are moving the history (today) or pushing the composer under the keyboard (breaks `#1839`). What "the reading row" means while the keyboard is up, and where the arrival goes, is the owner's call; the chosen rule is recorded here before the fix lands.

# Acceptance Criteria

- Phone, keyboard open, composer focused at the foot, light and dark: a live arrival moves neither the reading row nor the composer by more than 0.5 px (or 1.0 px on whole pixels), per-frame trace through the arrival, the held re-read and its landing.
- The arrival is reachable as the owner's ruling says (Newest chip count, or drawn below), and the Newest chip counts it.
- Keyboard closed, desktop, and the rule 370 traces unchanged (0.0 px); the reader's own sends still follow as before.
- Screenshot matrix (light/dark x phone/desktop x keyboard open/closed) with `BUILD.txt` naming the exact tree, and the advisory design review.

# Notes

- Found by the cl-arrival lane's evidence for `#2291` (kb-open cells), routed by coordb to cl-windowfoot with `#2292`.
