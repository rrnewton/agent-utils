---
title: 'team-restore-model-check: model-check every transition of the team-restore driver'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
- validation
created_at: 2026-11-12T11:38:25+00:00
updated_at: 2026-11-12T11:38:25+00:00
---

# Description

[opus 5.5] Owner, 2026-11-12: "We must be extremely careful about making this state machine explicit and even model checking all its transitions. It is very easy for us to introduce locking and paperwork but have many ways the process can fail and let nothing happen and things stall." See section 8 of the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).

# Design

- An explicit-state model of the driver (#1414), its unit restarts, N concurrent kickers, the team lead, a peer team (#1418), systemd and Herdr, with failure injection at every step: process death, Herdr down, a message lost or duplicated, a command timing out.
- Properties. Safety: at most one driver acting per team; no double launch; no revive of a parked or excluded agent. Liveness under weak fairness: eventually VERIFY passes or ESCALATE has delivered. Kick idempotence: any number of kicks in any state reach the same outcome as one.
- A self-contained checker in the repo (exhaustive search at small bounds), run by validate under the component's selection, robust on a hot machine (owner rule 346: no wall-clock budgets). The driver and the model share one transition table, and counterexample traces replay against the real driver as tests.
- Timebox (lead): model the minimal driver subset first so it can land; the full model gates the rest.

# Acceptance Criteria

- The checker finds a seeded bug (for example, a state with no timeout transition) and passes on the real table.
- The minimal subset's model passes before the minimal driver lands.
- A counterexample trace format that the driver's tests can replay.

# Notes

Numbered from devbig030's interim block (1400-1469). Part of the team-restore workstream; see the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).
