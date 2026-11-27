---
title: 'vibe-talk-reply-boxes-label-in-view: with the phone keyboard open, the boxes on screen carry no label because the first box (the only labelled one) sits under the status pill'
status: open
priority: 3
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-27T12:05:00-05:00
updated_at: 2026-11-27T12:05:00-05:00
---

# Description

A follow-up from the #2385 vibe-talk-thread-open-layout-first evidence review; it does not block that change.

The boxes that stand for a thread's replies on their way carry their words ("8 more replies loading") in the FIRST box only. On a phone at 412x915 with the keyboard open (viewport 579 px, composer focused), the page keeps the composer in view, so the list shows the lower boxes. The first box sits under the status pill at the top of the list. Every box the reader can see is then an empty dashed outline, with nothing saying what it is.

A screen reader is unaffected: the status line announces the words once, and the box list carries them as its label.

# Design

- Show the words in a box the reader can see. Two candidates:
  - put the line in the lowest box on screen (re-placed when the visible set changes, never per frame);
  - give the first box's line a sticky position inside the box list, so it rides under the pill's clearance while any box is in view.
- Either way the words stay one visible line (never one per box), and the screen reader is still told once.
- No layout change to the boxes themselves: their heights stay what the read's rows will take (owner rule 370).

# Acceptance Criteria

- Shots at 412x915, light and dark, keyboard open with boxes standing: the words "N more replies loading" are visible in a box on screen and not covered by the pill.
- Keyboard closed and desk shots are unchanged from #2385's.
- The thread reply boxes walk still says the line once to a screen reader and moves no row through the fill.

# Notes

Filed at the coordinator's request from the liaison's #2385 review (issue file only).
