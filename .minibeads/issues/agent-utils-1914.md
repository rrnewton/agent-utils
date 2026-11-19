---
title: 'agentctl-watch-heartbeat-before-first-pass: heartbeat begins at daemon start, not first pass'
status: open
priority: 1
issue_type: task
labels:
- agentctl
created_at: 2026-11-19T14:48:51-05:00
updated_at: 2026-11-19T14:48:51-05:00
---

# Description

[watermelon-full-preview] Finding (coordb, 2026-11-19 2:38 PM ET):
#1900 agentctl-services-repair-watch-first-pass-health (cde2597f)
made services refresh accept a kind=watch unit's health from an
advancing first-pass heartbeat in pass-progress.json. On one
registry's agent-watch it worked (3 min 55 s first pass, heartbeat
carried it). On the agentctl-agent-watch-agent-utils-014.service
unit it rolled back: report.json says "no healthy state within
120 s of the new process start: no watch health pass has completed
since the restart". That unit is pinned because its first pass
waits on wrkslots lock contention (#1808 wrkslots-create-starves-
under-heartbeat-churn). Hypothesis: the daemon blocks before pass
start (a lock wait, registry scan setup or startup), so
pass-progress.json is never written and the progress proof never
kicks in.

Correction (coordb, 2026-11-19, later): re-running the refresh
with the cde2597f snapshot's own agentctl went healthy ("watch
pass in progress, began 30 s ago, heartbeat advancing"), so the
2:38 PM ET rollback came from the refresher binary (the global
agentctl at 7678e58b, which predates #1900, exactly as its flat
120 s wording suggested), not from the daemon failing to write
its heartbeat; that unit's pin is removed. The heartbeat gap
below is still real: startup and lock waits before the first
pass have no heartbeat, so the fix proceeds at normal priority,
and the refresh report must name which health proof it applied
so a stale refresher is obvious next time.

Confirmed in this lane from code, the unit's argv and the rollback
record (refreshes/20261119T193815Z-cde2597ffe7d): the unit is
`agentctl watch --state-dir ...` (subcommand `watch`, discovered
as kind `watch`, not `watch-agents`), so the #1900 progress proof
does apply to its kind in both probe arms. `PassProgress` in
rs/agentctl/src/restore.rs only writes pass-progress.json at
`begin()` (pass start) and its heartbeat thread only writes while
a pass start is recorded; CLI startup before `restore::watch`
(Herdr client, config load, coordinator designation scan) and
startup inside both watch loops before the loop top, and the
agent-watch single-writer lock and state load before
`watch_agents`, all run with no heartbeat file from the new
process. A block in any of them leaves only a stale file from
before the restart, which health rightly rejects. The rollback
report's flat 120 s wording also shows the refresh runner that
produced it predated the #1900 proof, so the next refresh (run
with the landed binary) depends on this earlier heartbeat.

# Design

The heartbeat begins at daemon start and keeps advancing during
startup, lock waits and the first pass: `pass-progress.json`
gains `daemon_started_at` and a `phase` field (`starting` until
the first pass begins, then `pass`; reader skew reported to the
coordinator before gating). Each watch daemon writes the file at
daemon start (in the CLI arm, before client, config and
designation work, and again at the top of its watch loop before
state reads and, for watch-agents, before the single-writer lock),
with the heartbeat thread advancing `heartbeat_at` from daemon
start until the daemon stops. Services health accepts an
advancing, fresh heartbeat from a daemon that started after the
restart in either phase, and still accepts a completed pass and
#1900 pass-phase files as before, and its report detail names
the proof applied (`health proof completed-pass`,
`health proof pass-progress` or
`health proof daemon-start-progress`). A daemon stuck with a frozen
heartbeat still fails health and rolls back.

# Acceptance Criteria

- The refresh report's health detail names the proof it applied
  (`completed-pass`, `pass-progress` or `daemon-start-progress`
  for the watch kinds), so a refresher that predates a proof is
  obvious from its report.
- A test with a fake clock where a lock wait before the first
  pass (pass begins well after the health window, heartbeat
  advancing from daemon start in `starting` phase) refreshes the
  watch instead of rolling it back; it fails before the fix.
- A frozen heartbeat in the starting phase still fails health
  and rolls back, as a frozen pass-phase heartbeat already did.
- The daemon-side progress file is written before the first
  pass begins, shows `starting`, and its heartbeat advances
  before `begin()`; after `begin()` it shows `pass`.
- Existing completed-pass and #1900 progress proofs still pass
  for both watch kinds.

# Notes

[watermelon-full-preview] Related: #1900
agentctl-services-repair-watch-first-pass-health (the pass-start
proof this extends), #1808 wrkslots-create-starves-under-
heartbeat-churn (the lock contention the first pass waits on),
#701 agentctl-services-refresh. Services and both watch daemons
are Rust-only, so there is no Python mirror. After landing,
coordb refreshes the pinned unit and removes the pin.
