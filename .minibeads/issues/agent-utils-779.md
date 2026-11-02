---
title: 'agent-teams-host-scoped-allow: team@host entries in may_send_to and accepts_from'
status: open
priority: 2
issue_type: feature
labels:
- agent-teams
depends_on:
  agent-utils-773: blocks
created_at: 2026-11-02T17:19:48.879521886+00:00
updated_at: 2026-11-02T17:19:48.879521886+00:00
---

# Description

[opus 5.5] Owner policy (2026-11-02): cross-team traffic goes only between the two teams' designated leads on one host ('through the front door'); lieutenants and lanes on other hosts never message the other team directly. Declarations allow lists are team-level only, so today the rule is enforced by giving each host's declarations different lists (one host allows the cross-team pair, the other restricts both teams to themselves). Add team@host (and optionally role) entries to may_send_to and accepts_from, so one synced policy expresses 'only lead@host-A <-> other-lead@host-A', with clear refusal messages naming the rule. Related: #773 agent-teams-cross-team-route-refused (the refusal seen there was this team-level list).
