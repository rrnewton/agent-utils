---
title: 'composer-key-stay: Ctrl/Cmd+S keeps the view when the view has a message box'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-22T13:53:49.087025110+00:00
updated_at: 2026-10-22T16:06:12.194906082+00:00
closed_at: 2026-10-22T16:06:12.194905972+00:00
---

# Description

[opus 5.5] The owner, 2026-10-22: "when I press Cmd-S even if I have Main messages view on, it jumps to a DIFFERENT view such as All (Cmd 2) or Two column (Cmd 4). That is unexpected. I would want to keep my view and just enter the messages here." Cause: #403 composer-key-global leaves the thread list first (threadListBack) because the list used to cover the only box; #419 threads-view-main-messages made the main channel box the last card of the Threads view while Main channel is on.

# Design

The rule, restated for every view: Ctrl/Cmd+S never changes the view when the view on screen already has a box. Threads view with Main channel on: the box at the foot of the cards (or the #410 inline reply, where one is open). Threads view with Main channel off has no box, and keeps leaving for the view it was chosen from, as #403 did. Main, All, a thread, two columns: unchanged (they hold a box). Settings and Help are still left first: they are screens over the view, not views.

# Acceptance Criteria

Cmd+S in the Threads view with Main channel on focuses the foot box (or the open inline reply) and the layout stays list; with Main channel off it still goes back as before (negative control); the ? help line says so. Page tests per view; Chromium at 1280x800 and 393x852.
