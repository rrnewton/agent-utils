---
title: 'agent-teams-relay-slack-bullet-rewrite: the relay quarantines messages whose bullets Slack rewrote'
status: open
priority: 2
issue_type: bug
labels:
- agent-teams
created_at: 2026-11-23T08:05:00-05:00
updated_at: 2026-11-23T08:05:00-05:00
---

# Description

The agent-teams Slack relay quarantines messages from a confirmed sender because the body no longer matches its signed hash after undoing Slack link markup. Comparing a quarantined body byte-for-byte with the same message delivered by the direct route shows the only difference: every bullet character U+2022 followed by a space was rewritten by Slack to a hyphen and a space. Three messages were quarantined in one morning, each raising a repeating alarm, though nothing was tampered with.

# Design

- Undo the bullet rewrite before verifying, the same way link markup is undone: try the received body, then a candidate with line-leading or inline "- " restored to the original bullet where the signed hash then matches; accept only on an exact hash match, so no tampering can pass.
- Alternatively (or additionally), make senders encode bodies so Slack cannot rewrite them (for example normalize bullets to "- " before signing).
- Keep quarantining any body that still doesn't match.

# Acceptance Criteria

- A test with a signed body containing bullets, rewritten the way Slack rewrites them, verifies and is delivered; a body changed any other way is still quarantined.

# Notes

Found by comparing a quarantined message with its direct-route copy; filed by the coordinator.
