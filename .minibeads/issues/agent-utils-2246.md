---
title: 'vibe-talk-screenshots-48b-settle-closes-popover: the 48b capture''s settle step closes the bookmark popover before the shutter, and 48b/48e leave the bookmark set'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- screenshots
created_at: 2026-11-25T11:20:00-05:00
updated_at: 2026-11-25T11:20:00-05:00
---

# Description

Scene 48b-bookmark-clear-popover (from #2229 bookmark-chip-clear-popover) can file a picture without its subject. The capture checks the scene's expectations, which pass with the Clear popover open above the Bookmark chip, and then settles transient UI before the shutter. When the "Bookmarked" status pill is still up, settling clicks its dismiss control. That click is a press outside the popover, so the popover closes, as the feature intends, and the frame shows the chip stack with no popover. This was seen at laptop-1280, light and dark, from landed main, and it reproduced on a rerun. The earlier desktop frames had the popover only because the pill had already left by the time of capture.

Also, 48b and 48e leave the bookmark set on the shared throwaway server. A run of either scene alone across two themes then sets the bookmark with the same row's toggle in the second theme, which takes it away instead. The walk times out and the run exits 31.

# Design

Two changes, both in the screenshot harness and nowhere else. First, 48b and 48e are marked as scenes whose subject is transient UI (keep_transient), so nothing is settled away between their expectations and the shutter. Second, both scenes get an after hook that clears the bookmark through the popover's own Clear, so the server holds no bookmark when the next profile's walk starts. 48e restores the full viewport first, as it already does.

# Acceptance Criteria

- 48b at laptop-1280 and android-412, light and dark, shows the Clear popover above the chip in every frame.
- A run of only 48b and 48e across both themes and both profiles completes.
- The full phone and desktop screenshot walks still pass, with 48c, 48d and 49 reached.
- screenshots.py --self-test passes.

# Notes

Found while taking the AFTER shots for #2229 from landed main. The deployed feature isn't affected: real-input checks on phone and desktop passed 26 of 26.
