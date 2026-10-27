---
title: 'discord-live-reactions: a reaction added in Discord shows in vibe-talk within about a second, pushed over the Gateway'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-27T13:09:49.066452811+00:00
updated_at: 2026-10-27T13:09:49.066452811+00:00
---

# Description

Owner (2026-10-27): on the open-source installation connected to Discord, reactions do not reach vibe-talk. Slack got live reactions in #378 live-reactions; Discord has only REST reads (reactions show when a message is read) and, with live_poll_seconds set, a bounded re-read of a watched channel's newest window each tick.

Wanted: push-based reactions from Discord with low latency.

Rules:
- Subscribe over the Discord Gateway with the non-privileged GUILD_MESSAGE_REACTIONS and DIRECT_MESSAGE_REACTIONS intents only (no MESSAGE_CONTENT). Handle MESSAGE_REACTION_ADD, MESSAGE_REACTION_REMOVE, MESSAGE_REACTION_REMOVE_ALL and MESSAGE_REACTION_REMOVE_EMOJI, in configured channels and their threads.
- Publish each change through the same live-update path Slack reactions use (LiveHub::publish_update), carrying the message's full current reaction list, so an open page updates within about a second.
- Emoji map as REST reads already map them: unicode as itself, a custom emoji by its name with its id kept. Count per emoji; owner_reacted when the configured owner_user_id reacted.
- Heartbeat, resume on reconnect, re-identify when a session cannot resume, and reconcile a watched channel's recent window after a re-identify (events in the gap are lost). Stop on the close codes Discord marks as not reconnectable and fall back to polling.
- Opt-in per provider; Discord itself only (thread_api = native), never a bridge.
- Tests against a fake Gateway plus REST server: add, remove, remove-all, remove-emoji, custom emoji, reconnect with resume and with re-identify, 429 on the REST re-read.
- README: the bot permissions and intents to enable in the Discord developer portal.

There is no Discord server on the development host, so the first live test happens on the owner's own server: the work goes up as a draft PR (high-risk-preland-review) with a brief for an agent there.
