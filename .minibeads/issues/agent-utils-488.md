---
title: 'slack-app-agents: a Slack app''s posts can be named the coding agent, and who-is-who choices follow the owner to every device'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-24T10:44:32.700578398+00:00
updated_at: 2026-10-24T13:02:35.807075170+00:00
closed_at: 2026-10-24T13:02:35.807075040+00:00
---

# Description

[opus 5.5] The owner (rescued Slack message 1792593728, 2026-10-21, devbig014 Slack): "For slack channels do not have the ability to configure different account names that appear in the channel to map them onto coding agent identity or me or other? It looks like my coding agent messages from the herder chat bridge are registering as other and not rendering as the full width agent messages." On Slack every bot is drawn as another bot unless coding_agent_user_ids names its account (#323 slack-system-bots), and that list matches only the author id; a bridge's posts come from a Slack app, known by its app id. Settings' Who is who (moved back to the channel box by #402 fifty minutes after that message) lets the owner choose, but its choices stay on the one device, though #457 settings-server-side keeps every other Settings choice on the server.

# Acceptance Criteria

Who is who's choices sync through the server's client settings like every other Settings choice (#457 settings-server-side), so a coding agent named once on one device is drawn as the coding agent on every device; Help, the README and the example configuration say how to name a Slack app's posts (its one account, the id Who is who shows, in Settings or in coding_agent_user_ids). Page and server tests.

# Notes

[opus 5.5] Scope: a Slack app posts through one account (its bot user, or its bot id where a bridge gives no user), whatever display name each post carries, so the existing coding_agent_user_ids and the Settings choice already name it; what was missing for the owner was the list being per device (and, on 2026-10-21 14:42 UTC, in Messages: #402 moved it back to Channels at 15:34). Matching a Slack app id (A...) as such would need the app id carried on every message (52 struct literals, the message cache and the wire contract); left as a question for the owner rather than built.
