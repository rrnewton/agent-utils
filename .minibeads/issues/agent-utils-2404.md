---
title: 'vibe-talk-small-drag-reads-channel: offline-cache-phone "a 30px drag at the newest line read the channel" fails on unrelated gates'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- tests
created_at: 2026-11-27T15:58:35-05:00
updated_at: 2026-11-27T15:58:35-05:00
---

# Description

[opus 5.5, cl-signpost3] vibe-talk.check.offline-cache-phone-browser fails "phone: a 30px drag at the newest line read the channel" on gates whose changes do not touch vibe-talk. Examples: the #2344 gate on 252bf52d, and rows 88e09f7d and d09ed64e (2026-11-26). It is not on the known-red list. The ledger holds 76 rows failing this step, not all on this check.

The check, in tests/offline_cache_browser.py:
- At a phone size, it parks the reader at the newest line and pauses the page clock (pause_at, one second ahead).
- It waits for the request record to stand still over three 50 ms polls.
- It draws one finger up 30 px, short of the pull-to-refresh threshold.
- It waits for the record to stand still again, then requires no channel read since.

What 252bf52d's run recorded as the read during the drag: GET /api/v1/channels/<id>/timeline?view=main&limit=50&reason=poll. That is the page's own poll, not a pull. A pull reads with reason pull (view/channel/pull.ts).

Why it matters: the live page acts as the owner. So it must be shown whether a short drag can make the page read (or mark read) on its own.
- The source provider's read cursor moves only through POST .../upstream-read, an explicit write, and the mark after a reply (#300).
- A timeline read moves no read mark.

# Design

- Reproduce locally only, on a fake API and a release build of current main; never the live page. Run the step 20 times with load1 per run.
- Capture every channel read and every read-mark request, with its reason and the touch/scroll events before it.
- Classify:
  - a read caused by the drag itself (reason pull, or any read the drag triggers): a product bug. Fix the rule (a deliberate reader act only) with a test that cannot race (events, not timers).
  - only the page's own poll landing inside the window: a test fix under owner reply 346. Count only reads the drag can cause, and wait on events, not a still request record.

# Acceptance Criteria

- 20 local runs on main classified, with every read's reason and trigger named.
- If a product bug: the rule fixed, and a race-free test that fails before and passes after.
- If a test race: the check counts only drag-caused reads, waits on events, and passes 20/20 on a busy host (load1 recorded). A negative control, a page whose 30 px drag reads, still fails it.
- The deployed build compared with main for the code involved.

# Notes

Filed at the coordinator's request (liaison 1076), issue file only, before the repro. Related: #1512 (the stable-record wait), #2400 (the same class of fixed-window count in the strip walk).
