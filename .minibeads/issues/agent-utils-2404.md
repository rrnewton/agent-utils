---
title: 'vibe-talk-small-drag-reads-channel: offline-cache-phone "a 30px drag at the newest line read the channel" fails on unrelated gates'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- tests
created_at: 2026-11-27T15:58:35-05:00
updated_at: 2026-11-27T18:38:55-05:00
closed_at: 2026-11-27T18:38:55-05:00
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

## Resolution

A test race, not a product bug. Fixed by deec642d (vibe-talk: the pull-to-refresh checks count what the page issues, not what arrives late). Test only; the page is unchanged.

Classification, from 25 local runs on main with a fake API:
- The page issued nothing between the 30 px drag's touchstart and the next gesture, in 25/25 runs.
- No read mark (POST /read or /upstream-read) in any walk, server side or page side.
- The poll cadence is exactly 60 s of page clock, so the drag adds no poll.
- One reason=poll read is issued at the paused instant just before every drag, by the check's own clock.pause_at catch-up.
- The old check counted requests by arrival after a record that had stood still for 3x50 ms. That poll could arrive late on a loaded host and be counted as the drag's read.
- The deployed builds carry the same page code as main, so the live page does not read or mark read on a small drag.

The fix:
- The pull checks count what the page issues, in its own order: every channel read and read mark, with its reason.
- They wait on events: requests answered at task boundaries, and a gesture done one MessageChannel task after its touchend. These are tasks, so the paused clock holds none of it.
- A 30 px drag must issue nothing. The 160 px pull must issue exactly one read, reason pull.

Proof:
- With every poll reaching the fake API 400 ms late, at load1 258-268, the old check failed with the gate's exact message and the new one passed.
- A page whose pull arms at 20 px fails the new check.
- Fix runs: 29/30 phone runs, plus 10/10 in alternating pairs with the old check.
- Gate on 4aa55aaa: red only on discuss-browser (known red) and scroll-stability-browser (off-path). Both passed solo reruns on the same tree, then it landed.

Noted, not filed: one fix run failed an earlier, unrelated step, "phone: the room under the composer never caught up with the chip row". That is a 30 s wall-clock wait in the jump step's wait_room. It did not recur in 10 alternating pairs. If it recurs, it gets its own owner-reply-346 issue.
