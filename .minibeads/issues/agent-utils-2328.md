---
title: 'slack-relay-channel-shared-caller: two hosts list the shared relay channel as one Slack caller and keep tripping its per-conversation limit'
status: open
priority: 2
issue_type: bug
labels:
- chat
- slack
- agent-teams
created_at: 2026-11-26T19:35:00-05:00
updated_at: 2026-11-26T19:35:00-05:00
---

# Description

Both hosts' agent-utils chat bridges serve the shared agent-teams relay channel. Each lists it with `conversations.history` in its relay poll (interval clamped to the floor) and in its general rescan, and both list as the same Slack caller through the same client app. The Slack API proxy limits each caller to bursts of 30 `conversations.history` calls per conversation, then 3 a minute, so the two hosts together keep going over it. The current plugin logs (2026-11-26) show 570 rate-limited listings of the relay channel on one host and 128 in about 10.5 hours on the other. The host listing budget (#690, #1892) is a file under the user's runtime directory, so it coordinates the bridges of one host only; each rate limit then holds that host's general rescan for 90 s (the hold lines behind #2327). Owner channels are listed first, so owner delivery is slowed but not lost; relay delivery and the owner's other Slack tools pay the cost.

# Design

Options, cheapest first:
- The general rescan skips, or lists only every Nth pass, the relay channels whose bot posts the relay poll already covers (person posts there are carried by the event stream; the rescan is only their backstop).
- A per-conversation call budget in the plugin that follows the proxy's documented allowance, with a configured share per host for a conversation more than one host reads (for example half each), and a relay-poll floor derived from it.
- Longer term, the proxy's own advice: subscribe to changes instead of re-reading the channel.

# Acceptance Criteria

- Over 24 hours with both hosts running, rate-limited listings of the relay channel are near zero on both hosts, and relay messages still arrive within about a minute.
- A test of the per-conversation budget, or of the rescan's relay skip, shows the planned call rate.
- The plugin log names the conversation each spent call went to, so the split can be checked.

# Notes

Found in the #2108 debugging pass (owner request 392, 2026-11-26). Related: #1682 (slack-rate-limit-budget), #1892, #2327.
