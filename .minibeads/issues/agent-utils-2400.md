---
title: 'vibe-talk-strip-swipe-post-count-timing: the all-thread-strip walk counts every POST in a fixed 700 ms window, so a freeze report from a hot host fails it'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- tests
created_at: 2026-11-27T15:08:55-05:00
updated_at: 2026-11-27T15:08:55-05:00
---

# Description

[opus 5.5, cl-signpost3] `tests/all_thread_strip_browser.py` checks that a finger drawn sideways from a thread strip writes nothing. It does so by counting EVERY POST the fake API receives across a fixed `page.wait_for_timeout(700)` after the swipe. That is the timing-sensitive pattern owner reply 346 rules out of gating checks:

- The page's freeze watch (`view/shell/freezewatch.ts`, #2066) posts `/api/v1/freeze-report` on the next 250 ms heartbeat after any long task of 200 ms or more, at most once per 5 s.
- A hot host makes such long tasks; the fixture's own `post()` docstring already says so.
- A freeze report that lands inside the 700 ms window fails the check: "a swipe from the strip posted something". No swipe write is involved.

Found on the #2395 owner gate (row 1795807615020057618-38a1d2f6, 552bfbe7): the only red was "phone-412-light: a swipe from the strip posted something".

- The same tree passed 5/5 alone, and base 63d6aa43's page passed 5/5.
- In those runs the only POSTs the phone walk made were freeze reports; one followed a 247 ms long task on main.
- One 300 ms long task scheduled in the page just before the swipe reproduces the failure deterministically, on 552bfbe7 and on main alike. Its POST is `/api/v1/freeze-report`.
- The gate ledger holds 67 rows failing this step on many trees, and it is not on the known-red list.

# Design

- Count only writes a swipe could make. Allow-list the routes of Done, archive, dismiss and upstream-read (the doneact/outbox routes), instead of every POST. Telemetry (freeze report, scroll trace) no longer counts.
- Replace the fixed 700 ms with a condition. After touchEnd, wait until the gesture's handlers have run: two animation frames plus a macrotask. Then wait until the page's write path is idle, with no request in flight to an allow-listed route.
- Keep the row checks (not archived, no details, no transform) as they are.

# Acceptance Criteria

- The 300 ms long-task repro before the swipe fails the old check and passes the new one.
- Negative control: a swipe that really writes (a scratch page variant whose strip swipe archives, or a Done posted in that window) still fails the new check.
- No `wait_for_timeout` remains in the swipe check; every wait is a condition.
- The walk passes 5/5 on a busy host (load1 recorded per run).

# Notes

Filed at the coordinator's request (issue file only) after the #2395 gate row; the evidence is in the #2395 strip-red notes. Related: #2066 (freeze watch), #1118 (fixed-frame settles in browser checks), #1250 (vibe-talk timing robustness), #2373.
