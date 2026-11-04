---
title: 'watch-notice-backoff: the undelivered-owner-chat notice backs off, is timestamped, and replaces its pending copy'
status: open
priority: 0
issue_type: bug
assignee: cl-start
created_at: 2026-11-04T15:21:36.245953428+00:00
updated_at: 2026-11-04T15:21:36.245953428+00:00
---

# Description

The owner (2026-11-04, via the lead): watch-agents' notice to a coordinator, "the owner's chat messages to NAME are not being delivered ... This notice repeats every 5 min until they are delivered", has no backoff and no timestamp, and the copies piled into the coordinator's pane: "Not timestamped either and every 5 min without backoff is crazy".

# Design

- Apply #830's warning backoff to this notice: repeat at 1, 3, 6 and 12 intervals, then every 36; reset when the reason changes.
- Each notice carries the time (ET) and the age of the undelivered owner message(s).
- Never queue a repeat while the previous copy is still undelivered in the coordinator's queue: replace it instead.

# Acceptance Criteria

- Tests with a fake clock cover the backoff schedule, the reset on a changed reason, the timestamp/age text, and replace-not-append while a copy is pending.
- Gate passes; landed alone (P0).

# Notes

P0 by owner direction; assigned to the #830 author (cl-start), ahead of #839.
