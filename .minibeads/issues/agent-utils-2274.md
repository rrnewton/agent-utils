---
title: 'vibe-talk-phone-send-pill-overlaps-composer: on a phone with the keyboard open, the floating Send pill overlaps the channel composer lower-right corner'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
- ui
created_at: 2026-11-25T21:44:00-05:00
updated_at: 2026-11-26T10:10:00-05:00
---

# Description

On a phone with the on-screen keyboard open and the channel composer focused, the floating Send pill (`#keyboard-send`, the Send that stands at the keyboard's top since #1504 ios-keyboard-send) is drawn over the composer's lower-right corner. It covers the box's right border and corner, sticks out past the box's right edge, and hides whatever sits in that corner of the box: a round control's outline shows under the pill.

Seen in the shared screenshot harness's scene `77-composer-keyboard-open` at the `android-412` profile (412x915 at scale 2.6, viewport shrunk to 1520 device px for the keyboard), in both themes. The shots are `dark--android-412--77-composer-keyboard-open.png` and `light--android-412--77-composer-keyboard-open.png`, from the landed main of #2264 (`f25f33af`, page build 40343d12c422ef73d4ccc3e807da63a4). The same scene at `laptop-1280` has no pill and is clean. The keyboard-closed phone scenes show the composer's own action row (Send raw message, Cancel, Send) under the box, with no overlap.

Reproduce: `cd vibe-talk && scripts/run.sh --screenshots --theme both --profiles android-412 --only 77-composer-keyboard-open --out <dir>`.

# Design

- Find where the keyboard Send is placed against the focused field (the screen-keyboard module's corner Send and its placement styles) and why, in the channel composer, it lands inside the box's lower-right corner rather than clear of it.
- Place the pill so that it never covers the focused box or its controls: beside or below the box within the shrunk viewport, or reserve room for it in the box (as #1871 did for the compact layout's chip reserve). Keep the #1504 guarantee that a Send stays reachable above the keyboard.
- Check the same placement for the other boxes the keyboard Send serves (thread reply, inline reply, the Discuss ask box), since #1680 reports a similar overlap in the inline reply form.

# Acceptance Criteria

- At 412x915 with the keyboard open (viewport really shrunk) and the channel composer focused, the keyboard Send overlaps no part of the composer box, its border or its controls, in light and dark.
- The keyboard Send is still visible and works above the keyboard (the #1504 behavior), and the laptop-1280 composer is unchanged.
- A page test at phone size asserts the pill's rect does not intersect the focused composer's rect while the keyboard is open.
- Evidence: scene 77 at android-412 and laptop-1280, light and dark, with BUILD.txt naming the commit.

# Notes

Found by the #2264 screenshot review (owner request 374, part B) and filed at the liaison's request: an owner-UI bug, not fast-track. Related: #1504 (the keyboard Send itself), #1871 (the compact layout's chip reserve and keyboard Send), #1680 (open: two Sends overlap in the inline reply form while typing).

## Resolution

Fixed by `#2277 vibe-talk-key-scroll-freezes-11-26` (fbe206a9, 2026-11-26). The fitted composer (the channel's box, and its right-column twin) now rests whole on the keyboard's top, its own row with Send included, instead of parking that row under the keys. With the box's own Send in sight, the corner Send stands down, so nothing floats over the composer's corner and exactly one Send stands. The #1504 promise holds: a Send stays reachable above the keyboard (the box's own).

Checks: `tests/save_draft_chip_browser.py` (phone, keyboard open: no corner Send, the chip and the box's own Send clear of the text area), `tests/reply_keyboard_clutter_browser.py` (main composer: the row's own Send alone stands; the inline reply keeps its own single Send), `tests/geometry_restyle_browser.py` (no corner Send while typing at the foot) and screenshot scene 77 at android-412 and laptop-1280 in light and dark, before (main's page: the pill over the corner) and after (the whole composer on the keys, no pill), BUILD.txt naming the tree. The Discuss ask box was not re-checked here.
