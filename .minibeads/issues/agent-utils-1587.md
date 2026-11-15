---
title: 'agent-teams-relay-discord: carry agent-teams cross-host messages through a Discord channel like the Slack bridge'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-15T16:51:49.921963859+00:00
updated_at: 2026-11-15T16:51:49.921963859+00:00
---

# Description

## Goal
Route agent-teams cross-host messages through a Discord channel (#agent-teams-relay, channel id 1566504459499217097) the same way they already route through the Slack bridge. The owner wants a consumer team's two hosts to use it: the first host's bot has user id 1545796614826889226, and the peer host's bot has user id 1532416065114607829.

## What already exists
- rs/agent-teams/src/relay.rs is provider-neutral: "A provider only has to report the poster's stable sender ID and let a bridge post into the channel; Slack, Discord, or the deterministic fake all qualify". Envelopes travel in a fenced JSON block, receipts are posted in the envelope's thread, and ledger `accept` dedupes.
- The chat bridge has a Slack provider. `agentctl capabilities` lists `chat_subscriptions.reference_backends.discord-gateway` as implemented:false, with "gateway-subscription" and "live-end-to-end-verification" missing.

## What is missing
1. A Discord chat provider for the chat bridge:
   - subscription: Gateway, or REST polling as a first increment;
   - stable sender ids (Discord user ids);
   - posting into the channel and into a thread or reply for receipts;
   - escaping/unescaping for Discord markdown (the counterpart of undo_slack_link_markup).
2. Configuration: `agentctl chat init` for a Discord target, using a bot token from a file (never logged), the channel id, and the allowed peer bot ids.
3. Addressing (measured 11/15). Our bot wrongly ACKed an owner message that mentioned only the OTHER bot.
   - Admission must key on structured envelope targets and the bot's own user id or aliases, never on "I can see it".
   - Messages addressed only to another bot get no ACK. Duplicate and loop protection apply bot-to-bot.
4. A live end-to-end check between two hosts, plus a deterministic fake-provider test.

## Evidence
On 11/15 a Codex agent on the first host read and posted to channel 1566504459499217097 through the Discord REST API with the existing bot token (HTTP 200). Bot permissions in that channel are therefore fine.

## Related
- agent-utils-496 agentctl-team-messaging (closed)
- agent-utils-1490 agent-teams-relay-to-queue-delay (open)
- agent-utils-1553 codex-footer-composer

An agent on the first host is drafting a first cut on branch `discord-relay-transport` (draft PR in this repo; link to follow). The owning team should treat that as input, not as a design commitment.
