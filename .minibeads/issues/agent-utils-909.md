---
title: 'watch-sigterm-stop: agentctl watch stops promptly on SIGTERM'
status: open
priority: 2
issue_type: bug
assignee: cl-sync
created_at: 2026-11-05T19:17:54.745653100+00:00
updated_at: 2026-11-05T19:17:54.745653100+00:00
---

# Description

`agentctl watch` (the restore watcher run as a systemd unit, on 5c78da37) still ignores SIGTERM. It waits out systemd's 90 s TimeoutStopSec and is SIGKILLed on every stop or restart. `watch-agents` and `chat run` were fixed by #870 and stop in 14 ms and 0.0 s.

# Design

- Apply #870's stop-flag handling to `agentctl watch`: set a flag on SIGTERM and SIGINT, check it on every sleep, wait and timeout branch (including the per-interval wait and any retry back-off), and exit promptly with status 0.
- Log the stop timing (signal received to exit) the same way #870 does.
- A pass in progress may finish its current bounded step, but never starts another.

# Acceptance Criteria

- Test: SIGTERM during the interval wait exits within 1 s; SIGTERM during a pass exits after the current step, never after the next interval.
- A live check on a unit restart: stop time under 2 s, with no SIGKILL in the journal.

# Notes

Reported by the lead on 2026-11-05 at 2:17 PM ET (P2), from a consumer team's restore-watcher unit. The same fix shape as #870 (stop-flag work).

Assigned to cl-sync, to work on while its #892 gates run (the lead: any free lane).
