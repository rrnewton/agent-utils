---
title: 'team-topology-checkins: lead and subordinate teams check in every 6 hours, probe each other, and kick a downed peer'
status: open
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
