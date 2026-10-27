---
title: 'scrubber-full-width: on a phone the read strip spans the screen on the bottom row while a read plays'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-27T12:27:03.348070842+00:00
updated_at: 2026-10-27T12:27:03.348070842+00:00
---

# Description

[opus 5.5] The owner, 2026-10-27 05:10: "Look at the position of the scrubber and the width of it. Right now we have it not extend on the full width of the mobile screen and we also have it sitting above the second row of the control bar and above the little floating chips. For things like summary I don't like that. I want to conserve screen real estate and I would like the scrubber bar to the full width and the flush with the top of the bottom row of the controls. Those other controls you don't need to access while we are in active playback mode. So let's move it down and expand it. This will especially be good to do before we expand it for our multi-track scrubber."

Wanted, on a phone (coarse pointer) while a read of several blocks is up:
1. The strip (#read-steer: Previous, k of n, Next, the #380 timeline) spans the full width, respecting the safe-area insets, and stands flush on the dock's bottom row.
2. The bar's second row and the floating chips are hidden while the read plays, and come back when it pauses or stops (decide which, with car-mode-design's pause/stop split and the documents-view hierarchical navigator in mind).
3. Nothing overlaps (strip, bottom row, list); the list grows into the freed room.
4. A desk is unchanged unless it obviously benefits.

# Acceptance Criteria

Fake-DOM test of the layout state by playback state; a Chromium walk at 393x852 and 412x915 (touch), dark and light, with screenshots of playing, paused and stopped; the gate green.
