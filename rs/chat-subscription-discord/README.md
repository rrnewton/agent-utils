# Discord provider for the agentctl chat bridge

Draft provider for agent-utils-1587 (agent-teams-relay-discord): it carries
`agent-teams` cross-host messages through a shared Discord channel the same way
the Slack provider does. It adds **no** relay mechanism of its own: envelopes,
receipts and exactly-once delivery stay in `rs/agent-teams/src/relay.rs`.

* `agentctl-discord-plugin` (`chat-subscription.discord`) polls the configured
  channels over the Discord REST API and speaks `agentctl-chat-subscription` v1.
  Sender ids are Discord user ids. The cursor (`discord.v1;<channel>=<message>`)
  advances only when the bridge commits.
* `agentctl-discord-outbound` is the bridge's `outbound_command`. It handles
  `send`, which replies to the thread root (used for relay receipts) or posts at
  channel level, and `ensure_reaction`.

## Addressing (`src/addressing.rs`)

The bridge acts on a post only when its **text** @-mentions this host's bot
user id (`<@ID>`, compared as parsed snowflakes) or names a configured
`@alias`. Seeing a post, a reply-to, the API `mentions` array, or the bot's name
in text does not address it. A post addressing both bots is handled by each bot
for itself. A post addressed only to another bot draws nothing (the MSG-1 case
from 11/15). `<@BOT> agent=NAME@HOST: request` targets one agent: it is
delivered as the bridge's existing `!@NAME request` owner command. An unknown
agent, or another host when only this bot was addressed, is refused. Posts from
peer bots are relay posts for `agent-teams relay-in`, never prompts. This host's
own posts are never acted on.

## Configuration

`examples/bridge-newton-fedora.json` is an `agentctl chat init` configuration
for this host. The bot token is read from a dotenv file (`token_file`,
`token_key`; default key `DISCORD_BOT_TOKEN`) or from an environment variable
(`token_env`). The token reaches `curl` only through its standard input
(`curl --config -`). It never appears in argv, a log line or an error.

Discord limits a post to 2,000 characters, so set the relay route's
`max_post_bytes` to 2000 or less. The helper refuses, rather than splits, an
over-long post that contains a fenced block.

## Markdown (`src/markdown.rs`)

Discord stores text byte-for-byte; unlike Slack, it does not rewrite links, so
the relay needs no rescue on receipt. Visible text outside fences has its
mentions neutralised by default (`--text=mentions`). `--text=literal` escapes
markdown as well, and `unescape_markdown` is its exact inverse. Every post sends
`allowed_mentions: {"parse": []}`.

## Not yet done

The Gateway subscription, `ensure_rejection` with a human-author check,
honouring `retry_after` on 429, a dedicated rejection reason for refused
targets, true Discord threads, and a live two-host end-to-end check.
