---
title: 'watch-honour-stopped-timer: watch restarts a timer an operator deliberately stopped'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-11T08:23:34+00:00
updated_at: 2026-11-11T08:33:15+00:00
closed_at: 2026-11-11T08:33:15+00:00
---

# Description

[watermelon-full-preview] An operator deliberately stopped (did not disable) a user timer watched by `agentctl watch`, because each run of its service hit its CPU budget. A few hours later the timer was active again, and nobody on the operator's side had run systemctl on it.

The code confirms the watch as the restarter. In `rs/agentctl/src/restore.rs`, `timer_problem` maps any confirmed inactive timer (`LoadState=loaded`, `ActiveState` other than `active`) to `TimerProblemKind::ScheduleBroken`, and `watch_once` repairs that kind with `systemctl --user restart <timer>`, which starts an inactive timer. Nothing on that path reads `UnitFileState`, records that a stop was deliberate, or distinguishes "an operator stopped this" from "this timer's schedule is broken". The only throttle is the #961 agentctl-watch-timer-repair-loop repair cooldown (one repair per `max_age_seconds` window), which delays the restart; it does not prevent it. The same pass keeps reporting the triggered service's last failed run as a problem it could not repair, so the operator sees both the noise and the restart.

Live evidence on the reporting host, 2026-11-11 around 03:21 AM ET: the watched timer read `LoadState=loaded`, `ActiveState=inactive`, and its service's last run had failed hours after the operator's stop, consistent with the watch having started the timer again. The user journal was not readable from the investigating session, so the restart itself is established from the code path and the timer's state, not from journal lines.

# Design

- An inactive timer is an operator's stop until proven otherwise: `watch` never starts or restarts a timer whose confirmed `ActiveState` is `inactive`, whatever its `UnitFileState`. The finding reports it once per stop episode as stopped by the operator, naming the `ActiveState` and `UnitFileState` systemctl returned; later passes in the same episode carry the existing cooldown suppression, so neither stderr nor the coordinator alert repeats. The episode ends when the timer is seen active again.
- A timer may opt back into the old repair explicitly: a new `repair_stopped` boolean on its `timers` entry (default false). Only with that opt-in does an inactive timer take the existing schedule-broken restart path.
- Not-loaded timers and timers in a failed state keep today's confirmed-problem reporting; an active timer whose service failed on its last run is still reported and never repaired, exactly as today.
- `check_only` passes report a stopped timer without consuming the episode's one report and without any start.

# Acceptance Criteria

- Test with a fake systemctl: a stopped, still-enabled timer gets no `start`/`restart` call, is reported as stopped by the operator on the first pass, and is not re-reported on a second pass in the same episode.
- Test: the same timer with `repair_stopped: true` is restarted as before.
- Test: an active timer whose service failed is still reported and not repaired.
- Existing timer behaviour is unchanged: stale active timers still get one service start per cooldown window (#961), calendar timers are still repaired, and a timer that could not be checked is never repaired.
- The watch help text states that stopped timers are not started unless their entry opts in.

# Notes

Reported 2026-11-11 under the same-hour rule for agentctl defects. Builds on #961 agentctl-watch-timer-repair-loop (freshness by the service's last success, one repair per window) and sits beside #1145 repair-notify-pair (per-duty switches): honouring a stop is per-timer here, not a duty switch, because the stop is a fact about the unit, not about the daemon.

## Resolution

[watermelon-full-preview] Fixed in `rs/agentctl/src/restore.rs`: a confirmed inactive, loaded timer is `TimerProblemKind::StoppedByOperator` — reported once per stop episode as "stopped by operator, not started" (the row names the `UnitFileState`), with later passes in the episode carrying the cooldown suppression and no coordinator re-alert; the episode set is persisted in health.json as `timer_stopped_noted` and ends when the timer is seen keeping its schedule. A timer entry may set `repair_stopped: true` to opt back into the restart repair. Not-loaded and failed timers keep the schedule-broken path, and a failed service on an active timer is still only reported. Two new ScriptedHost tests cover the stopped timer (no start call, one report) and the opt-in restart; the restore suite passes 66/66.
