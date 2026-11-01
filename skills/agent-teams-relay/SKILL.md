---
name: agent-teams-relay
description: Activate and operate the agent-teams chat relay for one fleet - suggest the per-fleet channel, tell the owner what to create and whom to invite on Slack or Discord, propose the sender-to-team mapping from the bots visible in the channel, confirm peers, and handle loud stranger alarms. Use when a coordinator is asked to activate the relay, set up a relay channel, confirm or ban a relay bot, or respond to a stranger alarm.
---

# agent-teams relay

The chat relay is a third `agent-teams` delivery route through a shared
chat channel, beside ssh and node-exec. The owner can read the inter-team
traffic there. Words: a **team** is one host's agents under one
coordinator (`TEAM@HOST`); a **fleet** is all the teams across hosts on
the same repository (the name before the `@`).

The installed command is the authority: `agent-teams relay setup`,
`relay status`, `relay announce`, `relay teams`, `relay confirm`,
`relay ban`, `relay alarms`, and the "Chat relay" section of
`agent-teams userguide`.

## Activate the relay (a coordinator walks through this)

1. **Run `agent-teams relay setup --team <fleet> --provider slack|discord`.**
   It suggests the channel name and prints config snippets. Default
   layout: one channel per fleet, `#relay-<fleet>` (for example
   `#relay-agent-utils`), with only that fleet's teams' bots as members.
   A channel shared by several fleets also works; say so if the owner
   wants that instead.
2. **Tell the owner exactly what to do. Agents never create channels.**
   - Discord: create the text channel, invite each team's bot account,
     and give every bot permission to view the channel, read its history,
     and send messages. Each team may post as a different bot account;
     one bot per team is normal, and a team with two bots invites both.
   - Slack: create the channel, note its channel ID, and invite each
     team's Slack app (bot user). Each team posts as its own app.
   Ask the owner for the channel ID (Slack) once it exists.
3. **List the bots the channel can see and propose the mapping.** From
   the channel members the owner reports (or the bots that announce
   themselves once bridges run), propose `peer_senders`: each bot sender
   ID mapped to its fleet plus team, for example
   `{"U0BOT1": "sim@host-b"}`. One bot per team is the normal case;
   several IDs may map to the same team.
4. **Apply the three snippets from `relay setup`** (bridge
   `relay_channels`, this team's declaration `--relay`, and the
   `relay-coord` route, last in the target host's route list). On Slack,
   also name the same peers in the provider's backend configuration so
   peer bot posts are delivered to the bridge at all.
5. **Announce and confirm.** Each team runs `relay announce`. Peers show
   up as *candidates* in `relay teams` — never admitted yet. Run
   `relay confirm <sender-id>` for each: it verifies over a trusted
   route (ssh/node-exec) when it can, and otherwise asks for one-line
   owner approval (`--owner-approved`). Never admit an unconfirmed claim,
   and never put a peer ID in the bridge's `allowed_senders`: that list
   means the owner, and a peer must never be able to inject prompts.
6. **Verify with `agent-teams relay status`** and one low-stakes test
   message. Show the owner the sample post (tag line, body, fenced
   envelope block) and its thread receipt.

## Stranger alarms

An unconfirmed bot posting to your team is never silently dropped. Its
post is quarantined, and your coordinator gets a stranger-alarm notice
flagged for escalation to the owner through the normal owner channel.
Escalate it; do not sit on it. The alarm repeats (default every 30
minutes) until the sender is confirmed or banned:

- Legit new team bot: confirm it (`relay confirm <sender-id>`). Its
  quarantined posts are then delivered in order, exactly once.
- Unknown or hostile bot: `relay ban <sender-id>`. Its posts are dropped
  and the alarm stops.
- Posts for other fleets from confirmed senders are dropped silently by
  design (no tokens spent). Only strangers and malformed posts are loud.

## Rules

- Never create Slack or Discord channels, and never post to any channel
  except through `agent-teams send` over the configured relay route or
  `relay announce`.
- Relay posts are public to every channel member: combine points, send
  only what the other coordinator needs, no acknowledgements or chatter,
  and prefer a host route when nothing is urgent. `--private` messages
  never take the relay.
- Agents never write the team tag or the envelope block by hand;
  `agent-teams` renders both.
