---
title: 'guarded-kill-pid: a guarded kill helper that refuses the user manager, Herdr, foreign unit PIDs and other teams'' processes'
status: closed
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-11-12T11:38:25+00:00
updated_at: 2026-11-12T11:38:25+00:00
---

# Description

[opus 5.5] Root cause of the 2026-11-11 outage: an agent sent SIGTERM to a PID it took for a stale script's parent. It was the host's `systemd --user` manager, which stopped every user unit, and two teams were down (one for 15 hours). One team has adopted a manual rule; the lead asked for a mechanical one. See section 9 of the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).

# Design

- `agentctl kill-pid PID --expect-cmdline TEXT --expect-start TIME [--signal TERM]`: refuses unless the process's command line and start time match (so a reused PID is refused). Always refuses the user manager (`systemd --user`) and any `user@UID.service` process, the Herdr server, the main PID of any systemd unit the caller's team doesn't own, and processes whose cwd or cgroup belongs to another team's registry or slots.
- Prints what it refused and why, and logs every kill it performs to the team's state.
- The core team-restore skill and the coordinator skill tell agents to use it instead of a raw `kill`.

# Acceptance Criteria

- Fixture tests for each refusal (stubbed /proc and systemctl), plus a successful kill of an owned child.
- The skill text change lands in the same commit.

# Notes

Numbered from devbig030's interim block (1400-1469). Part of the team-restore workstream; see the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).

## Resolution

`agentctl kill-pid PID --expect-cmdline TEXT --expect-start TIME [--signal TERM]` landed in both agentctl editions (`py/agentctl/killpid.py`, `rs/agentctl/src/kill_pid.rs`, identical refusal reasons). It refuses unless the command line and start time match, always refuses the user manager, any `user@UID.service` main process, the Herdr server, foreign unit main PIDs, other teams' processes and anything the caller's team cannot prove it owns; the real killer re-verifies through a pidfd before signalling, and every kill is logged to `REGISTRY/kill-pid.jsonl`. Fixture tests stub /proc and systemctl with a fake killer (no real process is signalled): `py/tests/test_agentctl_kill_pid.py` and the Rust `kill_pid` unit tests cover each refusal plus a successful kill of an owned child and of an owned unit's main PID. The `team-restore` and `agent-utils-coordinator` skills now tell agents to use it instead of a raw `kill`.
