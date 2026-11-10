---
title: 'services-turnover: one host command that turns every agent-utils daemon over to main, only where its code changed'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-10T01:52:07.000000000+00:00
updated_at: 2026-11-10T01:52:07.000000000+00:00
---

# Description

[watermelon-full-preview] Owner request 2026-11-09 (reply 255): "Do we
have a smooth path for all teams, including 030 teams, to turn over to the
latest agent-utils / latest agentctl with restarting all daemons?" Today
there is no such path. `agentctl services build` plus `services refresh`
moves one services-managed unit at a time per registry (with backup,
health check and automatic rollback), and one host has a daily timer that
moves the global agentctl binary to main but leaves every service alone.
After the 2026-11-09 reboot, every daemon of both registries on the
reporting host was still running the same pre-reboot snapshot and nobody
had refreshed - there was no one place that even listed who was behind.

What exists and where it stops short:

- `services list` sees only services-managed units (bridges, the agent
  watcher, headless wrappers run as units). It misses the watch repair
  units, at least one bridge installed outside the services flow, and
  still lists a stale inactive headless-coordinator unit pinned to an old
  snapshot, so a refresh silently leaves units behind.
- Everything is per registry and per host, run by hand. A second host has
  no equivalent of the daily global-binary timer at all.
- The harness checkout, skill links, the terminal server, and tick timers
  all move separately; nothing reports them together.

Scope is OPT-OUT (owner reply 258, now a standing owner rule): the
default is that every agent-utils service upgrades and restarts. Only an
explicit pin - per unit or per registry, documented with a reason -
skips, and pins are listed with their reasons in every report. There is
no standing pin today (owner reply 260 lifted the last one as stale).
Turnover is change-aware (owner reply 274): a daily run restarts a
service only when the code it is built from changed since its running
build; otherwise it reports "unchanged since <sha>" and touches nothing.

# Design

One command per host, `agentctl services turnover`, owning discovery and
refresh in rs/agentctl/src/services.rs:

- Build the snapshot from main once per run (existing services build),
  then discover ALL agentctl units of every registry on the host -
  bridges, watch-agents, watch, restore, headless wrappers - by parsing
  user unit files for agentctl ExecStart lines and their --registry, not
  only services-managed ones. Other agent-utils daemons on the host
  (tick-hub timers and the like) are discovered the same way: refreshed
  when they run from a snapshot or checkout the build supersedes,
  otherwise reported.
- Change-awareness per service: compare the running snapshot's commit
  with main over the paths that affect that service (for agentctl
  services rs/agentctl and its workspace dependencies, plus the chat
  subscription plugin crates when in the snapshot). No affecting change,
  no restart; the decision and both commits are in the report.
- Refresh each changed, unpinned unit through the existing refresh path
  (backup, health check, automatic rollback), one at a time, stopping
  the run on the first rollback. Pins are small documented files (per
  unit or per registry) read before anything moves; a pin without a
  reason is refused as malformed rather than silently honoured.
- The global ~/.local/bin/agentctl moves in the same run with the same
  backup and rollback, gated by the same changed-paths comparison. The
  report states whether the daily global-binary timer is then redundant
  (expected: yes where a turnover timer exists; the timer template for
  hosts without one is a follow-up, reported in v1).
- `--report` is read-only: per service the running commit, the candidate
  commit, the changed paths, and the decision (refresh / unchanged /
  pinned with reason), plus stale or orphan units (missing binary or
  registry, superseded snapshot still installed), plus the global binary.

Seam with #1145 repair-notify-pair (coordinated through the lead): that
lane owns the watch daemon duties, the merged single daemon, and
generated unit names. This issue owns services.rs, discovery, the
turnover command and its report. Discovery must recognise all unit kinds
across the rename: the current repair unit (watch,
agent-watch.service, agentctl-agent-watch-*), the current notifier
(watch-agents), and the new combined daemon name once #1145 Phase 2
names it - without double-counting a registry that has both an old and a
combined unit during migration.

# Acceptance Criteria

- `--report` on a fake systemd user directory lists every agentctl unit
  class (bridge, watch-agents, watch old and combined, restore,
  headless, tick timer), each with running commit, candidate commit,
  changed paths and decision; writes nothing.
- A service whose affecting paths are unchanged since its running
  snapshot is not restarted and reports "unchanged since <sha>"; one
  whose paths changed is refreshed with backup and health check, and a
  health failure rolls it back and stops the run naming it.
- A pinned unit or registry is skipped and named with its pin reason in
  both report and turnover output; a pin file without a reason is
  refused before anything moves. With no pins present (today's state),
  every discovered agent-utils unit is in scope - no opt-in marker
  exists or is required.
- The global binary moves only when its build inputs changed, with the
  previous binary kept for rollback, and the report says whether the
  daily global-binary timer is redundant on this host.
- Stale and orphan units (missing binary, missing registry, inactive
  superseded snapshot) are reported by name with the exact removal
  remedy; turnover never deletes a unit file.
- Discovery lists a registry once even when old repair, old notifier and
  combined units coexist during the #1145 migration.

# Notes

Sequenced after the 2026-11-09 boot-restore landings (#952, #975,
#1137-#1140 on main). Live host application stays with the host operator:
this issue ships the command, its report, and tests against fake unit
dirs; the first live run is operator-run (a --report against the real
host is safe to run anywhere). The install-units timer template for
hosts without a daily turnover is a follow-up; v1 reports its absence.
