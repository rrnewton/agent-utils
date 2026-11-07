---
title: 'native-session-collision: several agent records bound to one native session block revive'
status: closed
priority: 1
issue_type: bug
assignee: mu-salvage
created_at: 2026-11-06T12:40:00.000000000+00:00
updated_at: 2026-11-07T02:15:00.000000000+00:00
closed_at: 2026-11-07T02:15:00.000000000+00:00
---

# Description

In one consumer registry, four killed lanes all record the same native session id as a fifth lane that is still live. As a result `agentctl revive` refuses each of the four with rc 75, and there is no supported way to point them at their own sessions.

# Design

- Find how several records captured one session. Check the capture added by #916 muse-native-session-id (9499d18c) and the older paths: bind-session, adopt, restore and revive.
- Repair route: `bind-session NAME <id>` with an explicit id must work on a dead record, or `revive NAME --session <id>` must exist, so an operator can point each record at its real session.
- Guard: one native session can never be bound to two live records. A capture or bind that would duplicate a live record's session is refused or recorded as unknown, naming the other record. A dead record that shares a live record's session is reported by status/doctor.

# Acceptance Criteria

- A test reproduces the collision through the capture path that caused it, and the guard prevents it.
- A test repairs a collided dead record with the explicit-id route, and revive then resumes the right session.
- Existing #916 tests keep passing.

# Notes

Reported 2026-11-06 by the lead coordinator, from a consumer's coordinator. That consumer's registry is read-only for this work; coordinate through the coordinator.

## Resolution

[watermelon-full-preview mu-salvage] Outcome. Landed as 76e1d1be. The collision came from older lanes started before #916 recorded native sessions, sharing one cwd and process tree, so a later discovery bound the one live session into several records. Capture now runs a guard (guard_duplicate_capture): a session already bound to a live record is left unknown on the new record, naming the owner and the bind-session remedy. Explicit bind-session works offline on a positively dead record (including agent_not_found), and the revive duplicate error names the bind-session remedy. Tests cover the capture collision, the dead-record repair, and revive after repair. Gate on the final commit passed every selected step (shared 38/39 with python.agentctl.shard-0 passing on exclusive rerun, exclusive phase 6/6).
