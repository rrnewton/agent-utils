---
title: 'service-sender-team: a --from-service sender is attributed to its own team, not the host''s first declaration'
status: closed
priority: 1
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T01:19:28.704170781+00:00
updated_at: 2026-11-05T03:08:32.760608842+00:00
closed_at: 2026-11-05T03:08:32.760608712+00:00
---

# Description

`agent-teams send --from-service NAME` attributes the message to the FIRST team declaration on the host, in sorted order (`rs/agent-teams/src/cli.rs`, the `--from-service` path), and ignores `--from-team` on that path. On a host that declares several teams, a service belonging to one team sends as another. Seen 2026-11-04: a consumer team's nightly alert service on a host where `agent-utils` sorts first would be labelled as coming from agent-utils.

# Design

- A service entry names its team: `hosts.HOST.services.NAME = {"unit": "...", "team": "TEAM"}`. Alternatively, `--from-service` honours `--from-team`. Pick one and document it; supporting both, with the entry winning, is fine.
- Refuse when the named team has no declaration on this host, or when the team's declaration does not list the service (for example a `services` list in the declaration). The refusal names the fix.
- An existing entry without `team`: keep today's behaviour only when exactly one declaration exists on the host; otherwise refuse with a clear "service NAME must name its team" error.
- The #843 allow list applies to the service's team.

# Acceptance Criteria

- Tests: a service with team T sends as T with several declarations present; a missing or undeclared team is refused; the single-declaration legacy case is unchanged; may_send_to is enforced for the service's team.
- `check` validates service entries; userguide and help updated; gate passes. Deploy on both hosts.

# Notes

P1 via the lead (2026-11-04). For the night of 11-04 the label is accepted as is. Assigned to mu-validate after #859.
