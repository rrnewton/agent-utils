---
title: 'vibe-talk-thread-pin-picker-slug-race: the pin picker check reads its rows once, before the thread summaries draw the named slug'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- tests
created_at: 2026-11-27T17:37:18-05:00
updated_at: 2026-11-27T17:37:18-05:00
---

# Description

[opus 5.5, cl-signpost3] vibe-talk.check.thread-pin-picker-browser failed on c3f49b05 (2026-11-27), whose tree contains #2158's fix (5d357dc9). Its rerun passed.

- The failure was a StopIteration at tests/thread_pin_picker_browser.py:430: no picker row carried the slug "release-day". The earlier checks had passed (three rows, chronological order, ages).
- The likely race (owner reply 346): the slug (the thread's display_name) comes with the view=threads summary read. open_popup waits only for three thread rows and two frames, then reads the rows once. When the summaries land after that, the named row has no slug yet.

From the frequent-red-steps audit, item P2. Its other reds were #2158's date bomb, not this race:
- All 18 reds from 2026-11-23 13:14Z to 11-24 01:09Z were on heads without 5d357dc9.
- Of the reds on heads with the fix, the only other one, 7c346872, was a tree with 45 failed steps.

# Design

- Reproduce deterministically: hold the fake API's view=threads read back (on the server thread, never the test's). The current check should fail with the same StopIteration.
- open_popup waits on the condition that the rows are complete (the named thread's .thread-picker-slug is drawn) before it reads them. The same for the thread header's slug check if it reads once.
- No fixed waits; a condition with the hang backstop.

# Acceptance Criteria

- The held-summaries repro fails the old check and passes the new one.
- Negative control: a fixture without display_name still fails the slug check.
- 10 runs pass on a busy host, load1 recorded per run.

# Notes

Filed at the coordinator's request from the audit's P2. A fix, not a disable or a known red (owner reply 346). Related: #2158 (the date bomb, fixed by 5d357dc9), #2175.
