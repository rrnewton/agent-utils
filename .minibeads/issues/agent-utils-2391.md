---
title: 'vibe-talk-thread-coverage-line-shifts-rows: the coverage line above a thread changes height when the whole thread lands, and at the top of the list nothing holds the rows'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-27T11:45:00-05:00
updated_at: 2026-11-27T11:45:00-05:00
---

# Description

Found by the #2385 vibe-talk-thread-open-layout-first walk. It is pre-existing and not caused by that change.

A thread drawn from the device's saved copy shows the coverage line "<thread> — the most recent messages; older ones are not loaded" above its rows. When the read that brings the whole thread lands, the line becomes "10 messages from <thread>", which is one line shorter on a phone (412x915) beside the search button. When the list stands at its top (scrollTop 0), nothing can hold the rows through that change: the root and every reply move up 17.4 px in one frame while the reader is looking at them. This breaks owner rule 370 (no Y-axis disruption).

Measured on the thread walk's fixture, phone, dark:
- With the #2385 boxes switched off (the old behaviour), the thread fills at its poll, about 45 s later, and the rows move 17.4 px.
- With the boxes on, the thread fills at once, and the rows move the same 17.4 px, all of it the line's own change. The fill itself adds 0.4 px or less.
- At 1280x800 the line does not wrap, and nothing moves.

`settleAfterRead` asks for the matching scroll correction, but at scrollTop 0 a browser has nowhere to scroll up to.

# Design

- The fix belongs to the coverage line, not to the read: its height must not change under a reader when the coverage it states changes. Candidate approaches:
  - keep the line's height (reserve its taller form while the view's coverage is still settling);
  - move it out of the scrolled flow;
  - hold the rows with room above (the same idea as the foot room #2367 used for the stop clamp).
- Whichever is chosen applies to every view whose coverage line changes when a read lands, not only threads.

# Acceptance Criteria

- On a phone at 412x915, light and dark, a thread opened from the saved copy at the top of its list and then filled by its read moves its rows at most 0.5 px on every frame.
- A check that fails on the current build (17.4 px) and passes with the fix.
- The coverage words stay honest at every moment.

# Notes

Filed at the coordinator's request from the #2385 evidence, issue file only. The #2385 walk (`tests/thread_reply_boxes_browser.py`) names this issue where it separates the line's own shift from the fill's.
