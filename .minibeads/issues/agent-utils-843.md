---
title: 'team-send-allowlist: agent-teams refuses cross-team sends outside a per-team allow list (front door enforced)'
status: open
priority: 2
issue_type: feature
assignee: mu-validate
created_at: 2026-11-04T18:49:03.833197782+00:00
updated_at: 2026-11-04T18:49:03.833197782+00:00
---

# Description

Owner rule (2026-11-02): an agent of one team may message the other team only through its front door. agent-utils agents may message only the consumer team's lead (`consumer-team@lead-host`), and the consumer team's agents only `agent-utils@lead-host`. (`consumer-team` and `lead-host` are neutral labels for the consuming team and the lead's host.) Today this is a convention: a team's declaration in teams.json has no per-team send allow list, so nothing stops an agent from addressing any team@host.

# Design

- A per-team@host send allow list in the team declaration (teams.json / declaration.json). An absent list keeps today's behaviour.
- `agent-teams send` and `reply` check the destination against the sender team's list and refuse a disallowed destination with a clear error, naming the rule and the allowed front door. They exit non-zero and nothing is queued or relayed.
- Same-team traffic (agent-utils@A -> agent-utils@B) is always allowed.
- Configuration for both teams, mirrored on both hosts: `agent-utils@*` -> `consumer-team@lead-host` only; `consumer-team@*` -> `agent-utils@lead-host` only.
- Decide, and document, whether a relay/serve hop re-checks the list or trusts the sender's host.

# Acceptance Criteria

- Tests cover: allowed cross-team, refused cross-team (with the error text), same-team always allowed, absent list = no change, and reply obeying the same rule.
- CLI help and userguide describe the allow list and its error.
- Gate passes. The rollout of the declarations on both hosts and both teams is coordinated through the lead (the consumer team's side goes through the lead, never directly).

# Notes

P2 by owner rule, via the lead (2026-11-04).

# Notes

## Done (mu-validate, 2026-11-04)

[muse watermelon-full-preview] `may_send_to` already existed at team granularity and was enforced in `cmd_send`; this change makes it team@host-granular (bare `team` = any host, `team@host` = only that host), checks it after destination resolution with a refusal naming the allowed front door, and documents that a relay/serve hop does not re-check (the sender's host admitted the envelope under the sender team's list). `reply` inherits the check via `cmd_send`. Tests: `may_send_to_allows_the_front_door_and_refuses_the_rest` covers refused cross-team (text + nothing queued), allowed front door, bare-team any-host, same-team always allowed, refused reply; absent list unchanged (existing round-trip tests). Help and userguide updated. Rollout diffs: /tmp/dev-agent-utils-handoffs/843-rollout.md (coordinator applies; no live declaration edited).
