---
title: 'team-topology-checkins: lead and subordinate teams check in every 6 hours, probe each other, and kick a downed peer'
status: closed
priority: 2
issue_type: feature
labels:
- agentctl
- agent-teams
created_at: 2026-11-12T11:38:25+00:00
updated_at: 2026-11-12T11:38:25+00:00
---

# Description

[opus 5.5] Owner, 2026-11-12: "I want a concept of a lead team and one or more subordinate teams ... an every 6 hour check-in if team a hasn't heard from team b already in that time ... the sub team to do a bit of a health check on the parent if it hasn't heard in 6 hours or has outstanding messages with no response for 60 min ... kick off recovery of the parent team like we did today as well as the reverse." On 2026-11-11 a team was down for 15 h and no peer noticed. See section 6 of the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).

# Design

- Topology in synced config: a team's lead team and subordinate teams (a team can be both).
- Check-in: if team A has had no message from team B for 6 h, A's driver sends B a check-in, which B's lead answers in one line.
- Peer probe: after 6 h of silence, or a message unanswered for 60 min, a team reads its peer's driver state and health remotely (not by asking an agent).
- Peer kick: if the probe finds the peer unhealthy or its lead unreachable, kick the peer's driver (idempotent, #1414), then tell its own lead and the owner once per incident.
- Remote access goes through the host's configured remote-exec command, never hard-coded.

# Acceptance Criteria

- Fixture tests: silence triggers a check-in; an unanswered message triggers a probe at 60 min; an unhealthy peer is kicked exactly once per incident even with several probers.
- Peer probe and kick are part of #1415's model.

# Notes

Numbered from devbig030's interim block (1400-1469). Part of the team-restore workstream; see the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).

## Progress (2026-11-12, partially delivered; issue stays open)

Landed on main in 3e646336 (`agent-teams: add topology fragments, checker and watcher decisions`), design in `ai_docs/plans/2026-11-12-team-topology-config.md` (9d7baf61):

- Per-team fragments `~/.config/agent-utils/topology.d/<team>.json` (schema `agent-utils-topology/v1`): identity, `registry`, `lead`/`subordinates` (no `peers` key), `coordinator_kind`; `team_key` computed on the target host as a state-directory name only.
- Checker: read-time merge of fragments with conflicts reported, never silently merged (duplicate instance, lead/subordinate mismatch, undeclared reference, undeclared `routes_override` route); watcher-own defaults precedence (own fragment, base, built-ins).
- Watcher decisions: 6 h check-in, probe after 6 h silence or 60 min unanswered, held-vs-unhealthy kick gating, exactly-once kick ledger keyed by `TEAM@HOST`, `PeerIo`/`tick()` seams. 9 fixture tests pass in the gate (`validate: OK`).

Remains:

- Live enablement: each team lands its own fragment from its own harness repo; live `team kick` against a real registry waits for the agreed drill window and the 05:30 ET turnover.

## Resolution

Closed 2026-11-12. The checker CLI landed in 59ed9126 (`agent-teams topology check`), and the real `PeerIo` adapter landed in 1479886f on the landed driver adcffec1 (#1416/#1417): `parse_driver_state` (frozen `agentctl-team-driver/v1`, incident treated as an opaque string, unknown fields ignored), frozen `team kick` and check-in argv builders, a real `ProcessRunner` (non-zero exit, including the host-namespace refusal, becomes an error carrying its reason), and per-target read-state argv templates from config. Topology tests 15/15 in the final gate (`validate: OK`). No live registry was kicked; enablement is per-team fragment landing plus the drill window.
