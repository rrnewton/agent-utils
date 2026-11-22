---
title: 'agent-utils-changelog-and-recovery-docs: a public CHANGELOG and one discoverable team-recovery guide'
status: open
priority: 1
issue_type: task
labels:
- docs
created_at: 2026-11-22T17:20:00-05:00
updated_at: 2026-11-22T17:20:00-05:00
---

# Description

An outside team lost its agents and did not recover with the supported flows. agent-utils has no CHANGELOG, and the recovery tooling (agentctl restore, the team-restore skill, the #1559 manifest, team act and run-agent-prompt) is not documented in one discoverable place; the team-restore skill does not mention the manifest, team act or run-agent-prompt.

# Design

(1) A public CHANGELOG at the repository root, reverse-chronological, with user-facing entries (recent landings at least back to the last month; a recovery entry). (2) One recovery guide (docs or a top-level RECOVERY section) for an outside team: what dies and why, what to run in order (dry run, real run, the force case after a user-manager restart), the manifest and team act, how to verify a team really came back, and what to collect when it does not; linked from the README and from skills/team-restore/SKILL.md, which is updated to cover the manifest, team act and run-agent-prompt. No host names, consumer names or private paths.

# Acceptance Criteria

- CHANGELOG and the guide are on main, linked from the README and the skill; a reader with only the public repo can adopt team recovery from the guide alone.

# Notes

Owner request; filed by the coordinator.
