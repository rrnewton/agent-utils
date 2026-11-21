---
title: 'agent-teams-service-reply-to: service sends need a routable reply_to'
status: open
priority: 1
issue_type: bug
labels:
- agent-teams
created_at: 2026-11-21T03:20:00-05:00
updated_at: 2026-11-21T03:20:00-05:00
---

# Description

[watermelon-full-preview] A --from-service send writes reply_to team svc.<service>, which has no inbox, so an ACK to a service alert cannot be routed. For service senders, reply_to.team must be the team the service is declared for, keeping from as the service for provenance. No free-form --reply-to flag.

# Acceptance Criteria

- A service send's envelope has reply_to = declared team@host; agent-teams reply to it routes to that team.
- Old receivers see a normal reply_to.
