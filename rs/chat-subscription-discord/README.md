# Discord provider for the agentctl chat bridge

Provider for agent-utils-1587 (agent-teams-relay-discord): it carries
`agent-teams` cross-host messages through a shared Discord channel the same way
the Slack provider does. It adds **no** relay mechanism of its own: envelopes,
receipts and exactly-once delivery stay in `rs/agent-teams/src/relay.rs`.

* `agentctl-discord-plugin` (`chat-subscription.discord`) polls the configured
  channels over the Discord REST API and speaks `agentctl-chat-subscription` v1.
  Sender ids are Discord user ids. The cursor (`discord.v1;<channel>=<message>`)
  advances only when the bridge commits.
* `agentctl-discord-outbound` is the bridge's `outbound_command`. It handles
  `send`, which replies to the thread root (used for relay receipts) or posts at
  channel level, `ensure_reaction` (followed by the ACK line, below), and the
  guarded `ensure_rejection`.

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
for this host. **It uses two channels.** The bridge treats a relay channel as
relay-only: anyone but a listed peer bot posting there, the owner included, is
refused (with the rejection reaction) and never typed. So the owner talks to
this host in an owner channel (`<OWNER_CHANNEL_ID>`, listed only in
`channel_ids`), and the two hosts' bots exchange envelopes in
#agent-teams-relay, which is in both `channel_ids` and `relay_channels`. The
bridge also requires the relay list to be repeated under
`backend_configuration.data.relay_channels` (channel and peer ids); the
plugin checks it against `peer_bots`. agentctl's test
`discord_example_config_admits_owner_requests_outside_the_relay_channel` admits
the shipped example through the real bridge.

The bot token is read from a dotenv file (`token_file`, `token_key`; default
key `DISCORD_BOT_TOKEN`) or from an environment variable (`token_env`). The
token reaches `curl` only through its standard input (`curl --config -`). It
never appears in argv, a log line or an error. Prefer a token used by nothing
else: another process using the same bot (a Claude session's Discord channel
plugin, for example) shares its rate budget and sees the same posts.

Discord limits a post to 2,000 characters. The helper refuses, rather than
splits, an over-long relay post (one with an `agent-teams-` fence), so set the
relay route's `max_post_bytes` to about 1900: neutralising mentions in the
visible text adds a few characters before the limit applies. Other replies are
split at line breaks, and a code fence crossing a break is closed and reopened
with its language, so long replies with code arrive whole. A one-line fence
(```` ```x``` ````) is ordinary text, and an opener line longer than half a post
is reopened as a bare fence.

curl 7.83 or later is needed for the rate-limit headers (`%header{}` in
`--write-out`); an older curl still works but reads only the 429 body.

## Markdown (`src/markdown.rs`)

Discord stores text byte-for-byte; unlike Slack, it does not rewrite links, so
the relay needs no rescue on receipt. Visible text outside fences has its
mentions neutralised by default (`--text=mentions`). `--text=literal` escapes
markdown as well, and `unescape_markdown` is its exact inverse. Every post sends
`allowed_mentions: {"parse": []}`.

## Rejections

A post addressed to this host is refused with feedback, the bridge's rejection
reaction, in two cases: the sender is not an owner (`sender_not_allowed`), or an
owner named an `agent=` target this host refuses (`target_refused`): an agent it
does not have (when `local_agents` is set), another host when only this bot was
addressed, or a malformed `agent=` header. The plugin log names which.

`ensure_rejection` reacts only after reading that exact message back from
Discord and finding a person as its author: the author id equals the request's
`sender_id`, the author is not marked `bot` or `system`, the post has no
`webhook_id`, and it is an ordinary message. Anything else answers
`author_not_human` and adds nothing. Each operation id is bound, before any
write, in the `--ledger` directory's `rejections/<id>.json`, so a retry cannot
target another message or sender and keeps its first emoji; a finished one
replays its receipt without contacting Discord. Without `--ledger` the action is
refused.

## ACK line (`--ack-host`)

The bridge acknowledges a request with a reaction once it has durably admitted
it, and Discord attributes a reaction only to the bot that added it. With
`--ack-host=HOST --ack-bot-user-id=ID` (plus `--ack-alias`, `--ack-bridge-agent`
and, if the bridge's acknowledgement is not 👀, `--ack-emoji`), the outbound
helper follows that reaction with one reply line naming the host and agent:
`ACK newton-fedora/lane-a: received`. It re-reads the post and applies the same
addressing rule to find the agent (`agent=NAME@HOST`, else the bridge agent; a
reply inside an existing reply chain names the host only, because the bridge
may route it to an agent bound to that thread). Because it is triggered by the
bridge's own acknowledgement, a post the bridge refuses never gets one. It is
best effort and carries no mention; relay posts get none. The helper reads the
request before reacting: if its acknowledgement is already there (the bridge
re-ran the operation), it posts no second line, however late, and reports
`already_present: true`. The read and the post share an 8 s budget, with
per-call timeouts cut to what is left and no rate-limit waits, so the line
cannot push the helper past the bridge's 25 s timeout.

## Peer-bot posts

A peer bot may post more than relay traffic in the relay channel. Only a post
with a line that opens an `agent-teams-` fence (envelope, receipt, announce),
or the relay's plain-text receipt, goes to `agent-teams relay-in`; that command
quarantines anything it cannot parse, so handing it a peer's chat would raise
an alarm for every reply. Prose that merely names the channel or a fence is not
relay traffic.

## Thread roots

A Discord reply chain is one bridge thread, keyed by its root message. The
plugin remembers recent roots; for a reply whose parent it does not know (after
a restart, for instance) it reads the parent chain back from Discord, at most 8
messages, so a restart does not split a thread.

## Robustness

A listed message that does not parse is checkpointed past, never fatal, so one
bad item cannot wedge the cursor. A rate limit or server error during the
initial baseline backs off like a poll (heartbeats continue meanwhile); only a
401 or 403 ends the subscription.

## Polling, latency and the rate budget

Each poll is one `GET /channels/{id}/messages?after=<cursor>&limit=100` per
channel; heartbeats make no call (the fake-provider test counts the calls). At
the default `poll_seconds: 5`:

| Quantity | Value |
|---|---|
| List calls per channel per bot | 12 a minute (0.2 a second) |
| Discord's global limit per bot | 50 a second |
| Time until a new post is seen | at most 5 s plus one round trip; 2.5 s on average |
| Owner to the other host's agent, via the relay | at most two poll intervals plus the bridge's own delivery |
| Posts per owner request | the reply's chunks, plus one ACK line with `--ack-host` |

`poll_seconds` accepts 2-60. Discord publishes the per-route bucket on every
response rather than documenting it, so the transport reads
`X-RateLimit-Remaining` and `X-RateLimit-Reset-After` and stops calling until a
bucket that reached zero has reset. A 429 holds every later call for its
`retry_after` without contacting Discord again. The subscription moves its next
poll to `retry_after` (never sooner than `poll_seconds`); other retryable
failures back off 30 s. The outbound helper waits out a `retry_after` of at most
5 s once and retries with the same nonce; a longer one is returned as
`rate_limited`, retryable, `not_applied`. Refusing early keeps the bot clear of
Discord's invalid-request limit (10,000 responses of 401, 403 or 429 in 10
minutes). A 401 or 403 ends the subscription instead of polling on. A reply whose thread
root is unknown costs up to 8 extra reads, once. These
figures follow from Discord's published limits
(<https://discord.com/developers/docs/topics/rate-limits>) and the call counts
above; no live traffic has measured them yet.

A Gateway (websocket) subscription would cut the latency to one round trip and
remove polling altogether. It is a later increment; nothing here depends on it.

## Discord permissions

The bot needs, in each configured channel: View Channel, Read Message History,
Send Messages, Send Messages in Threads and Add Reactions. Its application must
also have the **Message Content** privileged intent enabled in the Discord
Developer Portal: without it Discord returns empty text for posts that do not
mention the bot, which includes every relay post from the peer bot.

## Not yet done

The Gateway subscription, true Discord threads (replies stay Discord reply
chains rooted at the request, which the bridge's `!@NAME` binding uses as its
thread), and a live two-host end-to-end check. Inline code spans (single
backticks) still count as the owner's own words for addressing; fenced blocks
and quotes do not.
