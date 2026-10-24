---
title: 'read-aloud-recovery: a read whose ticket was lost prepares again and goes on, and a later tap works'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-24T10:44:31.012337187+00:00
updated_at: 2026-10-24T13:02:35.803134235+00:00
closed_at: 2026-10-24T13:02:35.803134135+00:00
---

# Description

[opus 5.5] The owner (rescued Slack message 1792793313, 2026-10-23 22:08 UTC, devbig030): long lag before reading starts after turning the cloud voice on; then a read "just died spontaneously before finishing the message and clicking it again. Got no response." The server log for 22:00-22:10 UTC shows: speech/prepare for the tapped message spent 45.9 s fetching it upstream (recent_hits=0, fetch_ms=45911) while the private Google Chat adapter was out of its read budget and its history reads were timing out; the server was restarted twice under the read (22:04:50 and 22:07:04 UTC, deploys); after the second restart the page asked for the read's old ticket three times and got 404 each time (tickets live in memory), and sent nothing else. The page ends a read when a streamed block fails and keeps every other ticket minted before the restart, so each later tap plays a dead URL and fails the same way (the toast in the screenshot: 'that message could not be played').

# Acceptance Criteria

A streamed part that fails (ticket gone, connection cut by a restart) forgets every prepared ticket and reads that part again on a fresh preparation, a bounded number of times with a short wait, saying so on the status line; a later tap after a failure prepares again rather than replaying a dead URL. Page tests for both; a Chromium walk with the fake server forgetting its tickets mid-read.
