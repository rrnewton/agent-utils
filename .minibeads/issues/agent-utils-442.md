---
title: 'read-strip-order: Play/Pause right of Next, a one-tap Stop, and a movable strip on a desk'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
- read-aloud
created_at: 2026-10-22T20:30:31.278992651+00:00
updated_at: 2026-10-22T21:19:58.793851849+00:00
closed_at: 2026-10-22T21:19:58.793851459+00:00
---

# Description

[opus 5.5] The owner, on #434 read-aloud-pause: the floating read-aloud strip read (Previous) (Play) 1 of 4 (Next), which is awkward. Put Play/Pause on the RIGHT of Next: (Previous) k of n (Next) (Play/Pause).

Scope added by the owner the same day: "it looks like the READ button on the bottom bar became a Pause button with no way then to STOP or exit read mode?! ... It would be ok if the Read button became a Pause + Stop button ... I would also be ok if on desktop I could move around that little floating controller by dragging." So: a Stop on the strip (after Play/Pause) and beside the bar's Read while a read plays, ending the read and leaving reading mode in one tap; on a desk the strip can be dragged by a grip, held in the window, remembered per device, and docked again by a double-click on the grip.

# Acceptance Criteria

On phone (393x852, 375 wide) and desktop the strip reads Previous, k of n, Next, Play/Pause, in DOM order as well as on screen (Tab and screen readers match). The strip keeps its 0px gap to the chips row and its wrapping. A page test pins the order; Chromium screenshots at 393x852 and desktop. The #429 keyboard-up slim row is checked for the same controls. Stop (strip and bar, phone and desk, also in the slim row) stops the audio and leaves reading mode through the existing path. On a desk the strip drags by a grip (pointer and keyboard), clamped to the window, remembered, double-click to dock; a phone keeps it docked. Tests and screenshots for both.
