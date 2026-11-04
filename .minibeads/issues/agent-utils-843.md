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

Owner rule (2026-11-02): an agent of one team may message the other team only through its front door. agent-utils agents may message only dev-hermit@devbig014 (that team's lead), and dev-hermit agents only agent-utils@devbig014. Today this is a convention: a team's declaration in teams.json has no per-team send allow list, so nothing stops an agent from addressing any team@host.

# Design

- A per-team@host send allow list in the team declaration (teams.json / declaration.json). An absent list keeps today's behaviour.
- `agent-teams send` and `reply` check the destination against the sender team's list and refuse a disallowed destination with a clear error, naming the rule and the allowed front door. They exit non-zero and nothing is queued or relayed.
- Same-team traffic (agent-utils@A -> agent-utils@B) is always allowed.
- Configuration for both teams, mirrored on both hosts: agent-utils@* -> dev-hermit@devbig014 only; dev-hermit@* -> agent-utils@devbig014 only.
- Decide, and document, whether a relay/serve hop re-checks the list or trusts the sender's host.

# Acceptance Criteria

- Tests cover: allowed cross-team, refused cross-team (with the error text), same-team always allowed, absent list = no change, and reply obeying the same rule.
- CLI help and userguide describe the allow list and its error.
- Gate passes. The rollout of the declarations on both hosts and both teams is coordinated through the devbig014 lead (the dev-hermit side goes through the lead, never directly).

# Notes

P2 by owner rule, via the lead (2026-11-04).
