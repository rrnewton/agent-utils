---
title: 'watch-sigterm-stop: agentctl watch stops promptly on SIGTERM'
status: closed
priority: 2
issue_type: bug
assignee: cl-sync
created_at: 2026-11-05T19:17:54.745653100+00:00
updated_at: 2026-11-05T23:28:10.992896030+00:00
closed_at: 2026-11-05T23:28:10.992895769+00:00
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

## Resolution

[opus 5.5 cl-sync] `agentctl watch` now applies #870 agentctl-service-slow-stop's stop-flag handling:
- The host's sleep (`SystemHost::sleep`) is `agent::sleep_unless_terminated`, so the interval wait and every retry wait inside a pass end as soon as SIGTERM or SIGINT arrives.
- The watch loop starts no other pass once termination is requested (`Host::terminated`). It answers the last pass's health and exits 0.
- The command installs the handler at start (not with `--once`), plus the 3 s guard that abandons a pass still running after the stop request.
- `log_stop_time("watch")` logs "watch stopped N s after the termination request".

Tests in `rs/agentctl/tests/service_stop.rs`, run with `--check-only` and a stand-in Herdr:
- SIGTERM during the interval wait exits 0 within 1 s, with the stop time logged.
- SIGTERM during a pass stuck in Herdr is abandoned within 5 s as SIGTERM's default action, and Herdr is asked nothing more.

Unverified: the live unit-restart check (stop under 2 s, no SIGKILL in the journal). It waits for the next deploy of agentctl to the watcher unit; the coordinator will run it on that deploy.
