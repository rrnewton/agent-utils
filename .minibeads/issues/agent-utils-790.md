---
title: 'agent-teams-relay-trust-key: trust a second team''s relay key without a bot mapping'
status: open
priority: 2
assignee: mu-mirror@devbig014
issue_type: feature
labels:
- agent-teams
depends_on:
  agent-utils-769: blocks
created_at: 2026-11-02T19:58:18.254307332+00:00
updated_at: 2026-11-02T19:58:18.254307332+00:00
---

# Description

[opus 5.5] 2026-11-02: 'relay confirm' always runs confirm_peer (one bot per team), so with one shared relay bot already confirmed for one team, a second team's key cannot be trusted through the CLI (relay-confirm-conflict happens before trust_key), and a 'relay announce' from that bot raises a one-bot-per-team alarm. Signed delivery already works with a shared bot. Ask: 'agent-teams relay trust-key TEAM@HOST', which reads relay_pubkey from the peer host's probe over the team's direct route (ssh/node-exec) and records only the key, without a bot mapping; the announce path stops alarming when a known shared bot announces a second team whose key is trusted. Blocks mirroring a consumer team's cross-host traffic, which the owner wants.
