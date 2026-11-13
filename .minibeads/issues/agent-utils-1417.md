---
title: 'team-restore-escalation: escalate an unrecovered team to a human through plain configured commands'
status: closed
priority: 2
issue_type: feature
labels:
- agentctl
created_at: 2026-11-12T11:38:25+00:00
updated_at: 2026-11-12T11:38:25+00:00
---

# Description

[opus 5.5] Owner, 2026-11-12: escalation "should be as low tech as possible ... use slack CLI to dm me. But at the agent utils layer of course it will need to be configurable and basically an arbitrary command. It could use multiple vectors for delivery ... don't escalate all the way to a banana phone." Configuration is synced, not per host ("cattle not pets"). See section 7 of the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).

# Design

- An ordered list of commands in synced config (`~/.config/agent-utils/escalation.toml`), each run with the incident text on stdin and `{host}`, `{team}` and `{incident}` filled in. No dependency on agentctl, Herdr or a chat bridge.
- Delivered = some command exited 0. One message per incident, at most one reminder an hour, one "resolved" message. If every route fails: retry with backoff, plus a local alarm file that `team kick` and `team verify` report.
- No paging or phone routes in the defaults.

# Acceptance Criteria

- Fixture tests: first route fails and the second delivers; all fail, giving backoff and an alarm file; the reminder rate is bounded; resolved is sent once.
- The escalation state is part of #1415's model.

# Notes

Numbered from devbig030's interim block (1400-1469). Part of the team-restore workstream; see the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).

## Resolution

[watermelon-full-preview] `team_escalate` delivers through ordered plain commands from the synced `~/.config/agent-utils/escalation.toml` (strict frozen subset): incident text on stdin, `{host}`/`{team}`/`{incident}` substituted in argv, first exit 0 delivered, no route distinct from failure. The driver keeps one delivery per incident, at most one reminder an hour, one resolved notice, backoff (60/300/900 seconds) plus `alarm.json` beside its state, which `team verify` already reports and `team kick` prints. Fixture proof: escalation module tests cover route order, substitution, all-fail and backoff values, and the atomic alarm; driver tests cover deliver-then-resolve once and alarm-on-failure. Escalation state is in the #1415 model and the persisted driver record. No paging or phone route exists anywhere.
