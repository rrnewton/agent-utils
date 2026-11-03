---
title: 'services-refresh-health-window-from-start: count the refresh health window from the new process start'
status: closed
priority: 3
issue_type: bug
labels:
- agentctl
created_at: 2026-11-02T23:21:25.022172767+00:00
updated_at: 2026-11-02T23:21:25.022172767+00:00
closed_at: 2026-11-03T18:42:07.419560+00:00
---

# Description

[opus 5.5] 2026-11-02: 'agentctl services refresh' starts its --health-timeout (default 120 s) at the restart call, but stopping an older bridge binary can take the unit's full TimeoutStopSec (90 s, plugin forced shutdown), leaving ~30 s for a fresh event on a quiet channel, so the refresh rolls back. Seen on a consumer bridge on one host (first attempt rolled back; a second with --health-timeout 600 succeeded). Fix: count the health window from the new process start (or extend it by the measured stop time), and say in the report how long the stop took.
