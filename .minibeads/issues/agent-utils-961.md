---
title: 'agentctl-watch-timer-repair-loop: watch restarts a monotonic-only timer on every pass because LastTriggerUSec never moves'
status: open
priority: 1
issue_type: bug
assignee: mu-validate
created_at: 2026-11-06T20:11:28.000000000+00:00
updated_at: 2026-11-06T20:11:28.000000000+00:00
---

# Description

`agentctl watch` repairs the timers listed in restore.json, and it judges a timer's freshness by `LastTriggerUSec` alone. Some timers trigger only on `OnUnitInactiveSec`/`OnUnitActiveSec`. For those, each repair restart resets the monotonic trigger, so the timer never elapses on its own. LastTrigger never moves, and watch restarts the timer on every pass, indefinitely.

Live case on a consumer host, 2026-11-06: an observer timer with `max_age_seconds = 1800` had LastTrigger stuck at a time before the host's reboot. Its service was replayed 141 times that day, each run using up to 600 MB of memory and writing 100-600 MB, where once every 10 minutes was expected.

# Design

- Judge freshness by the service's last successful run (`ExecMainExitTimestamp` with `Result=success`), or by the timer actually elapsing, not by LastTriggerUSec alone.
- Never restart a timer whose service ran successfully within `max_age_seconds`.
- If a monotonic-only timer really needs a kick, start the service once instead of restarting the timer, so the timer's own schedule is not reset.
- Rate-limit repairs per unit, and report each repair with the evidence that triggered it.

# Acceptance Criteria

- Tests with a fake systemctl:
  - a monotonic-only timer whose service ran recently is not touched;
  - a stale one gets one service start, not a timer restart;
  - repeated passes do not loop;
  - a calendar timer with a stale LastTrigger is still repaired.
- The watch report names each repair and why.

# Notes

Reported 2026-11-06 by the lead coordinator. P1 because it is burning real resources now. Assigned to mu-validate, which knows the watch code from #932 watch-agents-write-churn.
