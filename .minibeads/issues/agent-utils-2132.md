---
title: 'agent-utils-changelog-and-recovery-docs: a public CHANGELOG and one discoverable team-recovery guide'
status: closed
priority: 1
issue_type: task
labels:
- docs
created_at: 2026-11-22T17:20:00-05:00
updated_at: 2026-11-22T18:00:00-05:00
closed_at: 2026-11-22T18:00:00-05:00
---

# Description

An outside team lost its agents and did not recover with the supported flows. agent-utils has no CHANGELOG, and the recovery tooling (agentctl restore, the team-restore skill, the #1559 manifest, team act and run-agent-prompt) is not documented in one discoverable place; the team-restore skill does not mention the manifest, team act or run-agent-prompt.

# Design

(1) A public CHANGELOG at the repository root, reverse-chronological, with user-facing entries (recent landings at least back to the last month; a recovery entry). (2) One recovery guide (docs or a top-level RECOVERY section) for an outside team: what dies and why, what to run in order (dry run, real run, the force case after a user-manager restart), the manifest and team act, how to verify a team really came back, and what to collect when it does not; linked from the README and from skills/team-restore/SKILL.md, which is updated to cover the manifest, team act and run-agent-prompt. No host names, consumer names or private paths.

# Acceptance Criteria

- CHANGELOG and the guide are on main, linked from the README and the skill; a reader with only the public repo can adopt team recovery from the guide alone.

# Notes

Owner request; filed by the coordinator.

## Resolution

[opus 5.5, mu-recoverydocs] Delivered as documentation plus one validate routing line. `CHANGELOG.md` at the repository root lists user-visible changes newest first, one line each with its issue number, grouped by Sunday-to-Saturday week back to the week of 2026-10-18; `scripts/validate.py` classifies it as prose so later edits select no checks. The recovery guide is `common/docs/agentctl/recovery.md`: what kills a team, one restorer per team, the read-only look (`team manifest`), the restore dry run and real run (with when `--force` is still needed after #1405 made a user-manager restart a new epoch), `restore-teams` and `team kick` as alternatives, `team act` by hand with its incident and request-id rules, `run-agent-prompt` for a judgement, `team verify` plus the checks no command proves, known failures, what not to do, what to collect, and prevention (`install-units`, `shutdown`). It is linked from the README, `common/docs/README.md`, `skills/README.md` and `skills/team-restore/SKILL.md`, which now covers the manifest, `team act` and `run-agent-prompt`. Every documented command and flag was checked against `--help` of current main's Rust `agentctl` and Python `run-agent-prompt`. Nothing unbuilt is documented; the #1559 recovery agent gets its own section in a follow-up when it lands.
