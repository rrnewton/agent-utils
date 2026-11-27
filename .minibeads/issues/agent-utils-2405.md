---
title: 'vibe-talk-thread-card-header-font150-replies-button: thread-card-header-browser "phone-412-font-150: the replies button ... is not drawn" fails on unrelated gates'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
- tests
created_at: 2026-11-27T16:06:24-05:00
updated_at: 2026-11-27T16:06:24-05:00
---

# Description

[opus 5.5, cl-signpost3] vibe-talk.check.thread-card-header-browser fails "phone-412-font-150: the replies button of spaces/A/threads/runner is not drawn" on gates that do not touch the thread card. The step is not on the known-red list.

- The #2344 gate on 252bf52d (2026-11-27) recorded that exact message.
- Rows a4b8a857 (11-23), cbc7fd60 (11-23) and 4a0f6bf7 (11-24) also fail the step; their step logs are no longer kept, so their messages are not confirmed.
- The ledger holds 68 rows failing this step on many trees.

The case is a 412 px phone at 150% type: the thread card's header must draw its replies button.

# Design

- Classify first (for the red-audit lane):
  - Is the button really missing at 150% type (a layout bug: wrapped off, clipped, or zero size)?
  - Or does the check read before the header has settled (a test race under owner reply 346)?
- Reproduce locally only, repeated with load1 per run. On a failure, record the header's boxes and the button's computed display, size and clip.
- A layout bug gets a page fix with a geometry test. A race gets a check that waits on the header's own state, not a fixed settle.

# Acceptance Criteria

- The failure classified, with the evidence named (boxes, or the order of events).
- The fix, or the check made race-free, passing repeated local runs on a busy host (load1 recorded).

# Notes

Filed at the coordinator's request (issue file only) for mu-redaudit's classification. Low priority.
