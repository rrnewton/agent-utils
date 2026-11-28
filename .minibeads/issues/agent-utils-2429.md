---
title: 'vibe-talk-typing-never-lags: keystrokes and caret moves in any composer must cost O(1), with no list scan, row layout read or re-render'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- owner-ui
- perf
created_at: 2026-11-27T19:39:10-05:00
updated_at: 2026-11-27T23:09:06-05:00
closed_at: 2026-11-27T23:09:06-05:00
---

# Description

The owner, 2026-11-27 7:37 PM ET, verbatim: "the thing I hate most about this lagginess is when the TYPING is actually laggy within the text entry box".

From the owner's own Chrome traces of the live page (read in place; timings and function names only), as summarised by the liaison:
- Trace 2 (a slow reply): 26 long tasks over 50 ms, 9.6 s blocked.
  - The selectionchange listener ran 15 times for 3,168 ms. It fires on every caret move and keystroke.
  - One keydown took 1,563 ms. Layout took 738 ms and animation frames 645 ms.
  - Four clicks took 2,868 ms (the Reply taps, #2428).
- Trace 1: 9 long tasks, 2.4 s; selectionchange 413 ms.

The owner's lists run to about 100k px of rows, with windowed rendering off.

# Design

- The keydown, beforeinput, input and selectionchange handlers of every composer do O(1) work per event: no scan of the list's rows, no layout read across rows, no re-render.
- Anything else is deferred past the frame (after paint, or requestIdleCallback).
- Find what the document-wide selectionchange listener does (the quote selection chip, the read strip, find-in-page...). Cut it to selections that touch a message body, and to nothing while the caret is in a composer.
- Coordinate files with the windowed-default work (#2427) and the client-performance lane.

# Acceptance Criteria

- A fixture at the owner's scale (about 100k px of rows, windowed off), 100 keystrokes in the channel composer and the reply box.
- Script time per keystroke, p50, p95 and max, before and after: p95 under 8 ms. An input-to-paint measure, and counted work per keystroke (layout and recalculation events, rows touched) where a count can stand in for a clock.
- A walk that fails on main.
- Owner-UI evidence (phone and desk, light and dark, keyboard open and closed, Discuss open) with BUILD.txt.

# Notes

Filed at the coordinator's request (owner P0, fast track), issue file only. It shares a trace with #2428 (Reply open); they land separately.

## Resolution

Fixed by 39ac57f1 (vibe-talk: typing in a composer no longer relinks the whole list's glossary terms at every key). Deployed in 99a3399b; post-deploy guards green (typing_lag, glossary page tests, reply_continuity, inline_reply, search_unmatched).

- Cause: glossary.ts relinked the whole list on every collapsed selection (a composer's caret, so every keystroke) and on every list mutation (the reply box's own, the minute time tick).
- Fix: selectionchange links only roots a pass skipped while a selection stood. The observer links only rows added or whose message words changed.
- Measured, script ms per key at 1,216 rows: p95 98.5 -> 0.70 (composer), 98.8 -> 0.64 (reply box).
- Walk: tests/typing_lag_browser.py (dag 191). Main walked 9,691 text nodes at each key.
