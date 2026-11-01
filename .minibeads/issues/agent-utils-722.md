---
title: 'agent-teams-slack-relay: chat relay route via Slack/Discord (provider-neutral relay_channels)'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-01T06:48:05.653855135+00:00
updated_at: 2026-11-01T06:48:05.653855135+00:00
---

# Description

Give agent-teams a third delivery route through a shared chat channel (design ai_docs/transient/2026-10-24-agentctl-team-messaging.md sections 10.6/10.7/11): agent-teams relay route driver + relay-in, agentctl chat relay_channels admission class (peer posts never typed as prompts, dispatched only to configured command), narrow Slack provider switch for bot_message in relay channels. Provider-neutral: peer_senders maps sender id -> team (multiple ids per team, e.g. Discord vibing bots); admitted poster team must match envelope from.team; posts for other teams filtered by relay-in with no agent wake. Envelope authoritative in fenced machine-readable block with body hash; human-readable line above; receipts in thread, receipt never draws receipt; exactly-once from envelope ids + ledger. Test against fake chat provider and in-process tests; live channel #agent-teams-relay requested from owner, not created by agent.
