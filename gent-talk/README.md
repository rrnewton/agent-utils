# gent-talk

A small Rust web server that lets you talk to a voice agent about your coding agents' Discord
channels — from a phone, while driving — and, with explicit approval, post a reply back.

It is a **bridge, not an agent host**. It holds a Discord bot token, answers questions about
channels over an authenticated HTTP API, and serves a phone web app. It is deliberately **not**
co-located with the coding agents: it needs no access to any development workspace, and nothing in
it is tied to the machine it currently runs on. It starts in Podman on a laptop and is expected to
move to a small cloud host unchanged.

The design decision behind it, stated plainly: composing hosted products would also work (see
[`RELATED_WORK.md`](RELATED_WORK.md), which recommended exactly that), but a server is
needed either way to hold the Discord credential, and the owner would rather own that front door
than hand it to a vendor.

## What it does in v0

**Pull, not push.** There is no webhook receiver, no public ingress requirement, no signature
verification, and no "what if we weren't running" problem. When you ask, it fetches; between
questions it does nothing.

**Both summary and full text.** Speech cannot be skimmed, so a digest exists — one short line per
message. But the capability that actually matters is **semantic random access**: describe a message
in your own words ("the one about the mac runner") and get *that* message back **in full**. That is
`POST /api/v1/channels/{id}/resolve`, and it is the reason this exists rather than a
text-to-speech bot.

**A text tab.** The web app has a plain scrollback view for when you can look at the screen.

## Setting it up

**If you are setting this up for the first time, use [`QUICKSTART.md`](QUICKSTART.md).** It runs
the six steps — Discord bot, tokens, container, Cloudflare Tunnel, ElevenLabs agent, first
conversation — in order, with the exact commands and no gaps to fill in from here. This file is
the reference behind it: what every route does, what the security model is, and where it stops.

## Status: what works, what is stubbed

| Piece | State |
|---|---|
| HTTP server, routing, JSON API | **works** |
| Config file + environment overrides, with validation | **works** |
| Bearer-token auth, read scope vs write scope | **works** |
| **Per-request access log** | **works.** One INFO line per HTTP request, per MCP JSON-RPC message, and per tool call. No credential ever, no channel text above DEBUG. |
| Discord read + post behind a trait | **written, unit-tested; never run against live Discord** |
| In-memory Discord for tests and `--fake-discord` | **works** |
| Digest / summarization | **works**, extractive and deterministic (no model call) |
| Semantic random access (`resolve`) | **works**, lexical ranking behind a `Ranker` trait |
| Web app: text tab, digest, find-a-message, local speech | **works** |
| **MCP over Streamable HTTP at `/mcp`** | **works.** Bearer-authenticated, stateless, seven tools, tested end to end. Never yet driven by a real ElevenLabs agent. |
| ElevenLabs voice agent | **reachable, and currently NOT invoking tools.** A real agent has now been driven headlessly (`scripts/run.sh --smoke-agent`): the signed URL mints, the conversation opens, the agent answers — and it calls no tool, saying its tools "appear to be out of date". In the same conversation ElevenLabs reports our MCP server connected with all five tools visible, so the fault is in the agent's own configuration rather than in this server. |
| **Signed conversation URLs at `/api/v1/signed-url`** | **works against a fake, unverified against live ElevenLabs.** Mints a short-lived signed URL for an agent that has "Enable Authentication" turned on, and `/voice` is a dependency-free page that uses one. Tested end to end against an in-memory ElevenLabs that refuses a wrong key and an unknown agent, and against a loopback HTTP server that proves the account key travels in a header. |
| Slow path (ask a coding agent for detail) | **seam only.** The route exists and answers HTTP 501 with an explanation. |
| TLS | **not here.** Terminate in front (Caddy, nginx, a tunnel, or a cloud load balancer). A Cloudflare Tunnel recipe is below. |
| Agent smoke test (`scripts/smoke-agent.py`, `run.sh --smoke-agent`) | **works, and it goes red on the real failure.** Holds a real conversation and fails unless a tool invocation appears in this server's own access log. Manual and opt-in: it costs vendor minutes, so it is in no suite and no CI. |
| Deployment check (`scripts/verify-deployment.sh`) | **works.** One command, pass/fail, exits non-zero and names the failing check. Runs in CI against the container on every push, including a negative control that requires it to go red. |

Honest summary: everything except the two vendor-facing halves — live Discord and ElevenLabs — is
implemented and tested. Those two are exactly the parts that cannot be tested without credentials.

## What you must supply

Nothing is committed and nothing is hardcoded. You need:

1. **A second Discord bot**, separate from whatever your coding agents use, invited to the channels
   you want to reach, with `View Channel`, `Read Message History`, and — only if you want to post —
   `Send Messages`. Its token goes in `discord.bot_token` / `GENT_TALK_DISCORD_BOT_TOKEN`.

   > **Adding the bot to your server is not the same as adding it to the channel.** Authorizing
   > the OAuth2 invite puts the bot in the *server*; a **private** channel additionally needs the
   > bot, or a role it has, added in that channel's own **Edit Channel → Permissions**. Missing
   > that second step is the most common way this server ends up configured for a channel it
   > cannot read. The [startup channel probe](#the-startup-channel-probe) now catches it at
   > startup instead of letting it surface later as an empty digest.

   > **Turn on MESSAGE CONTENT INTENT** while you are in the Developer Portal: **Bot** tab →
   > **Privileged Gateway Intents**. It is off by default and it is the highest-cost thing to get
   > wrong here, because it does not fail — a bot without it is handed messages whose `content` is
   > **empty**, so the channel is found, the message count is right, and every digest line is
   > blank, with no error anywhere. It reads as a broken summarizer or a broken server.
   >
   > Scope, stated honestly: this server reads over the Discord **REST** API, and the documented
   > blank-content behaviour is on the **gateway** event path; we have never run against live
   > Discord and cannot tell you from experience whether REST is affected on your account.
   > Enabling it is one click and removes the question.
2. **Two API tokens of your own**, for this server's own callers:
   `openssl rand -base64 33` twice. They must differ and be at least 24 characters; the server
   refuses to start otherwise.
3. **The channel snowflake ids** you want reachable, with a label you can say out loud.
4. *(Later)* an **ElevenLabs agent id** and API key, when the voice half is wired.

Copy `gent-talk.example.toml` to `gent-talk.toml` (gitignored) and fill it in, or pass everything
through the environment.

## Running it

Locally, without touching Discord at all:

```sh
cd gent-talk
cargo run -- --config gent-talk.toml --fake-discord
```

`--fake-discord` serves in-memory channels seeded with a dozen deliberately long-winded agent
messages, and logs a warning on every start. It is how you look at the web app without a bot.
The seed is verbose on purpose: two cheerful one-liners make the digest and the scrollback look
like they work when neither of them had anything to do.

Against real Discord — **note that this is very nearly first contact.** The Discord client is
unit-tested and the whole server is end-to-end tested against an in-memory fake. Exactly one path
has been exercised against `discord.com` itself: the startup probe was run with a deliberately
invalid bot token and Discord answered **401**, which the probe classified and reported correctly.
Nothing else — no authenticated read, no post, and none of the 403/404 classifications — has ever
reached the real API. If something breaks here it is the untested seam finally being tested, not
a regression:

```sh
cargo run -- --config gent-talk.toml
```

Then open `http://<host>:8080/`, go to **Settings**, paste the read token, and press Save.

### Podman

Build (context is the `gent-talk` directory):

```sh
podman build -t gent-talk:v0 -f gent-talk/Containerfile gent-talk
```

Run with everything from the environment — no configuration file in the image, no secret baked in:

```sh
podman run --rm --name gent-talk -p 8080:8080 \
  -e GENT_TALK_DISCORD_BOT_TOKEN='your-bot-token' \
  -e GENT_TALK_READ_TOKEN='your-read-token' \
  -e GENT_TALK_WRITE_TOKEN='your-write-token' \
  -e GENT_TALK_CHANNELS='123456789012345678:lead team:rw,987654321098765432:build noise:ro' \
  gent-talk:v0
```

Or with a mounted configuration file (`:ro,Z` keeps SELinux hosts happy):

```sh
podman run --rm --name gent-talk -p 8080:8080 \
  -v ./gent-talk.toml:/etc/gent-talk/gent-talk.toml:ro,Z \
  gent-talk:v0
```

Behind a tunnel, publish to loopback only so the container has no path in except `cloudflared`:

```sh
podman run --rm --name gent-talk -p 127.0.0.1:8080:8080 \
  -e GENT_TALK_DISCORD_BOT_TOKEN='your-bot-token' \
  -e GENT_TALK_READ_TOKEN='your-read-token' \
  -e GENT_TALK_WRITE_TOKEN='your-write-token' \
  -e GENT_TALK_CHANNELS='123456789012345678:lead team:rw' \
  gent-talk:v0
```

The image runs as a non-root user and contains no configuration; a container started with no
credentials fails immediately rather than coming up with defaults.

**None of the commands above keep anything.** The image declares a volume at `/var/lib/gent-talk`
and points `GENT_TALK_STORAGE_PATH` into it, but a container run without a mount writes into its
own writable layer, which `podman rm` — and therefore every rebuild and every redeploy — destroys
without a word. Mount a host directory over it:

```sh
mkdir -p -m 700 ~/.local/share/gent-talk
podman run --name gent-talk -p 127.0.0.1:8080:8080 \
  -v ~/.local/share/gent-talk:/var/lib/gent-talk:Z \
  -e GENT_TALK_DISCORD_BOT_TOKEN='your-bot-token' \
  -e GENT_TALK_READ_TOKEN='your-read-token' \
  -e GENT_TALK_WRITE_TOKEN='your-write-token' \
  -e GENT_TALK_CHANNELS='123456789012345678:lead team:rw' \
  gent-talk:v0
```

`scripts/run.sh` does this for you: it creates `$GENT_TALK_DATA_DIR` (default
`${XDG_DATA_HOME:-$HOME/.local/share}/gent-talk`) mode `0700`, mounts it, and reports it under
`--status`. See **Durable state** below for what ends up in there.

## The startup channel probe

At startup, after the configuration is loaded and before the listener is bound, the server reads
**one message from each configured channel** and refuses to start if it cannot.

This exists because of a real failure. The server was configured, it started cleanly, it logged
its channels — and the bot had never been added to the channel. Nothing said so. The mistake
surfaced later as an empty result at first use, which reads exactly like a bug in this code.
That is a silent failure, and this project's rule is that an expected side effect which cannot
happen gets reported explicitly rather than skipped quietly.

**It only reads.** One message, per channel, with `limit=1`. It does **not** post, not even to a
channel configured `rw` — a startup check that wrote to your channel on every restart would be its
own bug. Nothing in `src/probe.rs` can reach the posting call, and a test asserts the in-memory
Discord recorded zero posts after a probe.

**It tells the causes apart**, because they have different fixes. Discord's numeric error codes,
not just the HTTP status, decide which message you get:

| What Discord says | What it means | What to do |
|---|---|---|
| 401 | The **bot token** is wrong, expired, or regenerated | Reset the token in the Developer Portal and update `GENT_TALK_DISCORD_BOT_TOKEN`. This one is global, so the probe stops after the first channel instead of repeating itself. |
| 403, code `50001` (Missing Access) | The bot **cannot see this channel at all**: either it is not in the server, or it has no `View Channel` there | Re-run the OAuth2 invite for the right server; then, for a private channel, add the bot or its role in **Edit Channel → Permissions**. Discord does not distinguish these two, so the message names both. |
| 403, code `50013` (Missing Permissions) | The bot **can** see the channel but may not read its history | Grant **Read Message History** on that channel. The invite is fine; this is a per-channel override. |
| 404, code `10003` (Unknown Channel) | Wrong snowflake, or a channel this bot was never invited to | Re-copy the id (Developer Mode → right-click → Copy Channel ID); if the id is right, re-check the invite. |
| 429 that did not clear | Rate-limited beyond the client's own retry budget, so readability was **not** established | Wait and restart. The client already waited the time Discord asked for; if it did not clear, something else is very likely sharing this bot token. See "Discord rate limits". |
| no response | The process cannot reach `discord.com` | Check egress: DNS, outbound HTTPS, proxy. |
| success, but every message has **blank content** | Almost certainly **Message Content Intent** is off | Turn it on (Developer Portal → Bot → Privileged Gateway Intents) and restart. |

That last row is a **warning, not a refusal**, and deliberately so: a message can legitimately be
attachment- or embed-only, so refusing to start on it would be a false alarm the operator cannot
clear. It is the one misconfiguration Discord reports as a *success*, so it is called out by name
rather than left to be discovered as "the digest is empty".

**Failure is fatal.** The server names every channel that failed, says why, says what to do, and
exits non-zero. That is the same posture the configuration already takes toward two identical
tokens or a token under 24 characters: for a single-user tool, a start in a state you did not mean
is worse than a refusal.

**Skipping it, for offline development:**

```sh
cargo run -- --config gent-talk.toml --skip-startup-probe
# or
GENT_TALK_SKIP_STARTUP_PROBE=1 cargo run -- --config gent-talk.toml
```

The skip is logged as a warning on stdout *and* printed on stderr, naming which switch caused it.
An invisible skip would be the same silent start the probe was added to remove. (`0`, `false`, and
the empty string do not skip, so a container runtime that renders an unset variable as `""` cannot
turn the check off by accident.)

Note that `--fake-discord` does **not** skip the probe, and the in-memory Discord does not fake a
pass: it knows which channels it actually has, and answers anything else with Discord's own
404 / `Unknown Channel`. If the fake said yes to every channel, the probe would pass against it
unconditionally and the tests would prove nothing about the real client.

## Discord rate limits

Discord answers an over-quota request with **HTTP 429** and says precisely how long to wait: a
JSON body carrying `retry_after` in **fractional seconds** and a `global` flag, a rounded
`Retry-After` header, and the `X-RateLimit-*` family. This client reads that and obeys it. The
whole of it is `src/discord/ratelimit.rs`, and it is shared by the live HTTP client and by the
in-memory fake, so the engine the tests exercise is the engine production takes.

**A 429 is no longer a caller's problem.** It is waited out and the request retried, so a question
asked during a burst is answered a fraction of a second later instead of failing.

**The waiting is bounded, twice**: at most four attempts and at most thirty seconds of total
waiting per request, whichever comes first. A wait longer than the remaining budget is refused
*before* it is taken — parking a caller for a minute and then failing anyway is the worst of both.

**Running out is loud.** The budget is not a way of failing quietly: it produces
`DiscordError::RateLimited`, whose message names the rate limit, the route, how many attempts were
made, how long was actually spent waiting and what Discord still wants. It is never turned into an
empty channel or a generic upstream error.

**Global and per-route are different things and are kept apart.** A per-route 429 shuts one
bucket; a `global` one shuts the whole bot token, and every channel waits behind it. Filing a
global limit under the channel that happened to observe it would let the next channel spend a
request walking into the identical wall a millisecond later.

**A request that is certain to be rejected is not sent.** A 429, or a *successful* response whose
`X-RateLimit-Remaining` is `0`, closes that bucket until it resets; the next request on it waits
rather than being spent. Where Discord's `X-RateLimit-Bucket` says two routes share one bucket,
they share one gate.

**The poll loop's backoff and this are one mechanism at two scales, not two mechanisms.** The
client's wait is the inner, precise one Discord asked for; only a limit it could not clear reaches
`src/live.rs`, and that loop's per-channel doubling then waits *at least* as long as Discord's
outstanding request. The outer wait can be longer than Discord asked for, never shorter.

**All of this is tested against the in-memory fake, and only against the fake.** The fake can
return Discord's 429 with a `Retry-After` and can carry the bucket headers on a success, and the
tests measure the waits on a paused clock. The header names, the body field and the rounding
behaviour come from Discord's published documentation; none of it has met live Discord.

## Checking a deployment

One command, against a running server, local or public:

```sh
scripts/verify-deployment.sh --url http://127.0.0.1:8080 --channel <your-channel-snowflake>
```

It takes the tokens from `$GENT_TALK_READ_TOKEN` / `$GENT_TALK_WRITE_TOKEN`, or from
`--read-token` / `--write-token`. Because it takes the base URL as an argument, the **same** checks
run against `http://127.0.0.1:8080` right after `podman run` and against
`https://<your-tunnel-hostname>` once `cloudflared` is up — if the second disagrees with the
first, the tunnel changed something.

What it asserts, in this order:

| # | Check |
|---|---|
| 0 | `GET /healthz` answers — nothing below means anything otherwise |
| 1 | unauthenticated `POST /mcp` → **401**, and the body names no tool, channel, service, or protocol revision; `GET /mcp` → **405** |
| 2 | read token `tools/list` succeeds, offers the four read tools, and **does not contain `post_reply`** |
| 3 | read token calling `post_reply` by name → **HTTP 403**, JSON-RPC **-32001** |
| 4 | a channel outside the allowlist → refused with `unknown_channel` |
| 5 | read token `digest_channel` → your **real messages**, non-empty |
| 6 | write token `post_reply` → accepted, and the message is then **read back out of Discord** |

It exits non-zero on the **first** failure and names the check that failed; there is no partial
pass and no check that can silently no-op. Failures carry the likely cause — an empty digest, for
instance, points at the Message Content Intent rather than just reporting an empty string.

`--skip-post` drops checks 6 and 7, which are the ones that put a real message in a real channel.
Using it means the half of the system that speaks in your name is untested.

The script is not shipped untried: the gent-talk CI workflow runs it against the built container
on every push, and then runs it three more times in conditions where it **must** fail (a channel
outside the allowlist, a read-only channel, a wrong token) and fails the build if any of those
passes. A check that cannot go red certifies nothing.

If you would rather do it by hand, the two calls that matter most are:

```sh
# 401, and the body says nothing useful
curl -s -o /dev/null -w '%{http_code}\n' -X POST localhost:8080/mcp \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'

# the read token's tool list — post_reply must NOT be in it
curl -s -X POST localhost:8080/mcp \
  -H "authorization: Bearer $GENT_TALK_READ_TOKEN" \
  -H 'content-type: application/json' -H 'accept: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

## Configuration

| Setting | File | Environment | Notes |
|---|---|---|---|
| Bind address | `server.bind` | `GENT_TALK_BIND` | `0.0.0.0:8080` in a container |
| Public URL | `server.public_base_url` | `GENT_TALK_PUBLIC_BASE_URL` | informational |
| Time zone | `server.timezone` | `GENT_TALK_TIMEZONE` | IANA name, default `UTC`; an unknown one refuses to start |
| Count ceiling | `discord.max_count_scan` | — | how many messages a count may walk, default `500` |
| Live poll interval | `discord.live_poll_seconds` | `GENT_TALK_LIVE_POLL_SECONDS` | seconds between inbound reads per channel; **`0` (default) is OFF**, and under `5` is refused |
| Discord bot token | `discord.bot_token` | `GENT_TALK_DISCORD_BOT_TOKEN` | **secret** |
| Read token | `auth.read_token` | `GENT_TALK_READ_TOKEN` | **secret**, ≥ 24 chars |
| Write token | `auth.write_token` | `GENT_TALK_WRITE_TOKEN` | **secret**, ≥ 24 chars, must differ |
| Channels | `[[channels]]` | `GENT_TALK_CHANNELS` | `id:label:rw` / `id:label:ro`, comma separated |
| ElevenLabs agent id | `elevenlabs.agent_id` | `GENT_TALK_ELEVENLABS_AGENT_ID` | public |
| ElevenLabs API key | `elevenlabs.api_key` | `GENT_TALK_ELEVENLABS_API_KEY` | **secret**, needed to mint signed URLs |
| ElevenLabs API base | `elevenlabs.api_base` | `GENT_TALK_ELEVENLABS_API_BASE` | `https://api.elevenlabs.io/v1` |
| Storage path | `storage.path` | `GENT_TALK_STORAGE_PATH` | absolute; unset means **no durable state** |
| Conversations kept | `storage.max_conversations` | — | default `50`, oldest dropped first |
| Turns per conversation | `storage.max_turns_per_conversation` | — | default `1000`, then `413` |
| Cached summaries kept | `storage.max_summaries` | — | default `2000`, least recently written dropped first |
| "Dealt with" marks kept | `storage.max_dismissals` | — | default `10000`, oldest dropped first; **not** bounded by age, see below |
| Retention age | `storage.retain_days` | — | default `30`; `0` means no age limit, and it bounds cached summaries too |
| Summary threshold | `summaries.threshold_chars` | — | below this nothing is summarised, default `400` |
| Summary width | `summaries.target_chars` | — | default `160` |
| Summary context | `summaries.context_messages` | — | preceding messages shown as context, default `3` |
| Summary model | `summaries.model` | `GENT_TALK_SUMMARY_MODEL` | unset means the extractive backend |
| Resuming | `replay.enabled` | `GENT_TALK_REPLAY_ENABLED` | **off by default**; on, every new call re-sends earlier conversation content to the vendor |
| Replay budget | `replay.max_chars` / `replay.max_turns` | `GENT_TALK_REPLAY_MAX_CHARS` / `_TURNS` | default `6000` / `40`; oldest dropped first; `0` is refused, not "unlimited" |
| Replay transport | `replay.transport` | `GENT_TALK_REPLAY_TRANSPORT` | `contextual_update` (default) or `client_data` |
| Config file path | — | `GENT_TALK_CONFIG` | or `--config` |

Environment wins over file. An empty environment variable is treated as unset, so a runtime that
renders unset variables as `""` cannot blank out a configured value. An unknown key in the file is
an error, not a shrug — a typo'd section name should not silently disable a setting.

## Durable state

Almost nothing in this server is remembered. A channel is never cached as a channel: every
question is a fresh Discord fetch, and it stays that way. There is exactly one store,
`src/store/`, and it holds these:

* the `/voice` **conversation transcript** — what the owner said and what the agent said back;
* **read marks** — how far the owner has been shown each channel;
* **"dealt with" marks** — which individual messages the owner has finished with, which is the
  overlay `#50 todo-view` is built on. Two snowflakes and an instant per row, and **no message
  text at all**; and
* **cached summaries** — one short line per long message, filed under the policy that produced
  it. This is the one entry NOT authored by this server: with the shipped extractive backend a
  summary is literally the opening of somebody else's message, so it is a second at-rest copy of
  third-party text and is treated as one everywhere below; and
* **channel aliases** — what THIS app calls a channel, one row per configured channel at most.
  Ours and local in the same sense the read marks are: never sent to Discord, renaming nothing
  there. See "What to call a channel". `#39 channel-alias`.

The first three are the owner's own record. The last is a derived cache, bounded by the same
retention as the rest, erased by the same purge, and never filled by a read-scope token.

**Every table is bounded on write, and one of them is bounded differently on purpose.** The
"dealt with" marks have a count ceiling and **no age limit**, unlike everything else here. An age
limit would put a message the owner cleared last month back into his to-do list because time
passed — a lie he cannot diagnose, since nothing on the row would say why it came back — whereas
an expired summary is merely regenerated. The count bound is enough on its own here for a reason
it is not enough for a summary: this is the one table that holds nobody's words.

### Read state is ours, and is never synchronised with Discord

Discord does not share read state with bots. There is no ack route a bot may call, no read-state
field on the channel object a bot can see, and no `read_state` in the gateway `READY` payload for
a bot user (this is what issue #61 `unread-status` established). So the read marks here are
gent-talk's own record, and the rule is stated once rather than left to be inferred:

* **No sync-in.** Marking a channel read in the Discord app changes nothing here.
* **No sync-back.** Marking a channel read here acks nothing and posts nothing; the Discord app's
  unread badge does not move.

Anything that shows a read mark to a person has to say that, because "read" already means
something else to a Discord user.

### Where it lives

One SQLite file at `storage.path`, which **must be absolute**. A relative path resolves against
the working directory, which in a container is a directory in the *image* — a store like that
works perfectly until the next rebuild and is then gone, with no error anywhere. The server
refuses a relative path at startup rather than discovering this later.

Unset means **off**, and off is loud: the store is a `DisabledStore` that refuses every call and
names the setting to add, and the routes that need it answer `503 storage_not_configured`. It is
deliberately not a silent in-memory substitute, because state that quietly evaporates on restart
is worse than state that was never promised.

### What is kept, for how long

Retention is enforced **on every write**, not by a sweeper that might never run: at most
`max_conversations` conversations (least recently active dropped first), at most
`max_turns_per_conversation` turns in one of them (further turns are refused, not silently
dropped), at most `max_summaries` cached summaries (least recently written dropped first), and
nothing older than `retain_days` days — since its last turn for a conversation, since it was made
for a summary. The defaults are 50, 1000, 2000 and 30.

Two tables are deliberately outside all of it: the "dealt with" marks, which an age bound would
put back into the to-do list purely because time passed, and the **channel aliases**, which an age
bound would rename back months later with nothing on screen saying why. Both are bounded by
something other than time — a count, and the channel allowlist.

The age bound is the only thing that collects an **orphan**: a cached summary whose message was
edited or deleted upstream is unreachable by key, is filed under the current policy version, and
nothing will ever announce that it went. The startup sweep cannot help — it deletes by policy
version, and an orphan is under the live one.

### How an operator purges it

Three ways, all of which erase everything — transcripts, read marks, "dealt with" marks,
cached summaries and channel aliases alike:

1. `DELETE /api/v1/storage` with the **write** token, on a running server. This is the only
   complete erase over HTTP: `DELETE /api/v1/conversations` clears transcripts and deliberately
   leaves the read marks alone, because the two are different records;
2. delete the file (`rm ~/.local/share/gent-talk/gent-talk.sqlite3`) — with the server stopped; or
3. delete the mounted directory, which is also what `scripts/run.sh --status` prints.

Unsetting `storage.path` is **not** one of them. It turns the feature off for the next run and
leaves every byte already written exactly where it is — which is the opposite of a purge, and is
worth saying because it looks like one.

### Security posture — this is new, and it is a change

This is the first thing gent-talk retains at rest, and what it retains is the owner's own speech
plus Discord text written by third parties.

* The database file is created `0600` and its directory `0700`.
* It is **not encrypted**. Anyone with filesystem access to the host — another process running as
  the same user, a backup, a stolen disk — can read every transcript. There is no key management
  here and pretending otherwise would be worse than saying so.
* The bind mount means state now **outlives the container, the image, and a token rotation**. A
  rebuild no longer clears it. That is the point, and it is also the risk.
* Message content still never reaches the access log at INFO; see below.
* Conversation ids are validated against `[A-Za-z0-9_-]{1,64}` before they are used as a key,
  because they arrive from the vendor and from the browser.

### Cached summaries

`src/summarize/` is a second trait over the same store. The shipped backend is extractive —
truncation, no model, no network, no cost — and the server says which one is running at startup;
every summary answer carries the backend name, so a page can never imply a model summary it did
not get.

What is actually load-bearing here is the **cache key**, because a cached derived value has one
failure mode and it is silent. Five things decide what a summary says: the prompt, the model, the
width, the context window, and the message text. The first four are folded into
`summarize::policy_version` — one function, one string, part of every key — so changing any of
them makes every summary produced under the old policy unreachable at once, and a startup sweep
deletes the entries. The fifth is a separate `content_hash`, so an upstream **edit** invalidates
one entry rather than the whole cache.

The version is deliberately legible (`v1-extractive-w3-c160-8f1b…`), because a stale entry is
diagnosed by someone with a shell. It is FNV-1a and is **not** a security hash: it detects a
configuration change, and nothing more.

A summariser is a model being fed channel text, so the request goes through `src/untrusted.rs`
exactly as the MCP path does — the instruction outside the fence, the message inside it. That is
structural rather than a convention: `SummaryRequest` holds the built prompt in a private field
and the only constructor builds it through `untrusted::fenced`, so a backend cannot be handed
channel text that was never framed, and a test reads back what the summariser was actually given
rather than rebuilding it. The shipped extractive backend uses neither the prompt nor the context
— it talks to no model, so there is nothing to fence it against — and both ride along for the
backend that replaces it. Being short is not an exemption.

A cached summary is a second at-rest copy of other people's text under the same file, with the
same `0600`. It is bounded by the same retention, it goes with the rest on a purge, and it is
never written by a read-scope caller: `/summary` answers a read token and serves it from the
cache, but only a write-scope caller's answer is filed. The read token is the one pasted into the
voice agent, and a durable write reachable from it is what the two-token split exists to prevent.

### Migrations

`src/store/sqlite.rs` holds an append-only `MIGRATIONS` list and records progress in SQLite's
`user_version`. Opening an older file applies the missing steps. Opening a file written by a
*newer* gent-talk is **refused**, naming the reason — a downgrade that wrote through the old
statements would corrupt data quietly.

## The access log

At `RUST_LOG=info` this server used to log **nothing per request**. That gap has a specific cost,
and it was paid: a voice agent reported that it had read a Discord channel and posted a reply, and
it had done neither — it invented a digest, and it named tools this server does not have
(`file_system_read`, `code_execution_sandbox`, `web_scraper`). The claim could only be disproved
by opening Discord and looking, because "the agent never called us" and "the agent called us and
we answered" produced identical output: none.

So every request now leaves exactly one line, and **an empty log is a finding rather than an
ambiguity**. The server says so in a banner at startup, because a reader has to know the log would
have spoken.

```text
INFO gent_talk::access: request method="POST" path="/mcp" credential="write" status=200 millis=12
INFO gent_talk::access: mcp rpc_method="tools/call" credential="write" is_notification=false
INFO gent_talk::access: tool tool="post_reply" channel="123…" credential="write" outcome="ok" reason="-" text_len=48
```

Filter for just these: `RUST_LOG=gent_talk::access=info`. Three lines answer the question that
started this: *did the agent call, which tool, which channel, and was it allowed?*

What a line never contains:

* **No credential.** Not the token, not a prefix, not a hash. Only the *class* that arrived:
  `absent`, `unrecognized`, `read`, or `write`. "Absent" and "unrecognized" are kept apart because
  they are different incidents — a misconfigured client versus a stale token or a stranger.
* **No channel text at INFO.** Message content is written by other people; it does not belong in
  an operator's log or in whatever ships that log onward. A post records its *length*, which is
  what answers "did the whole message go through?". The full arguments of a tool call are
  available at `RUST_LOG=debug` and only there.

A refused request is logged as loudly as an accepted one, including a request to a path that does
not exist — a client pointed at the wrong URL is exactly the case this log exists to catch.

## The API

All `/api/` routes require `Authorization: Bearer <token>`. `/healthz` and the static web app do
not, and neither reveals anything about the configuration.

| Method | Path | Scope | Purpose |
|---|---|---|---|
| GET | `/healthz` | none | liveness |
| GET | `/api/v1/channels` | read | configured channels |
| GET | `/api/v1/client-config` | read | what the web app needs at startup |
| GET | `/api/v1/agent-tools` | read | the voice agent's tool manifest and approval policy |
| GET | `/api/v1/signed-url` | **write** | mint a short-lived signed conversation URL — see below |
| GET | `/api/v1/channels/{id}/messages?limit=` | read | full scrollback, oldest first |
| GET | `/api/v1/channels/{id}/messages/{message_id}` | read | one message in full |
| GET | `/api/v1/channels/{id}/digest?limit=&width=` | read | one speakable line per message |
| GET | `/api/v1/channels/{id}/messages/{message_id}/summary` | read | one message summarised, from cache when it can be |
| GET | `/api/v1/channels/{id}/page?limit=&before=&since=&until=` | read | **one step of a walk**, saying that it is one |
| GET | `/api/v1/channels/{id}/count?since=&cap=` | read | a bounded, honest count |
| POST | `/api/v1/channels/{id}/resolve` | read | **semantic random access** |
| POST | `/api/v1/channels/{id}/reply` | **write** | post as the bot |
| POST | `/api/v1/channels/{id}/ask` | **write** | slow path — answers 501 in v0 |
| GET | `/api/v1/channels/{id}/stream` | read | **Server-Sent Events**: messages as they arrive — see "Live push" |
| GET | `/api/v1/conversations` | **write** | stored `/voice` transcripts, most recent first |
| DELETE | `/api/v1/conversations` | **write** | erase every stored transcript |
| GET | `/api/v1/conversations/{id}` | **write** | one stored transcript, oldest turn first |
| DELETE | `/api/v1/conversations/{id}` | **write** | erase one stored transcript |
| POST | `/api/v1/conversations/{id}/turns` | **write** | record one turn |
| GET | `/api/v1/conversations/{id}/replay` | **write** | that transcript, budgeted and fenced, for a new call — see "Resuming" |
| GET | `/api/v1/inbox` | read | how far this server thinks each channel has been read |
| GET | `/api/v1/channels/{id}/todo?limit=` | read | the recent window MINUS what has been dealt with |
| POST | `/api/v1/channels/{id}/dismiss` | **write** | `{messages:[…]}`, or `{through:id, limit:n}` — mark as dealt with; answers the exact set it changed. Send back the `limit` you read `/todo` with, or `through` is resolved against the default window and clears more than you displayed |
| POST | `/api/v1/channels/{id}/restore` | **write** | `{messages:[…]}` — the undo, restoring exactly that set |
| POST | `/api/v1/channels/{id}/read` | **write** | move this server's read mark forward |
| DELETE | `/api/v1/channels/{id}/read` | **write** | drop this server's read mark |
| PUT | `/api/v1/channels/{id}/alias` | **write** | `{alias:"…"}` — what THIS app calls the channel; see "What to call a channel" |
| DELETE | `/api/v1/channels/{id}/alias` | **write** | drop it, putting the configured label back |
| DELETE | `/api/v1/storage` | **write** | erase EVERYTHING durable: transcripts, read marks, "dealt with" marks, cached summaries, channel aliases |
| POST | `/mcp` | read, or **write** per tool | MCP over Streamable HTTP — see below |
| GET/DELETE | `/mcp` | none | `405`; this endpoint is stateless and has nothing to push |

**The transcript routes require the write scope even to READ.** A stored transcript is the
owner's own speech plus whatever channel text the assistant read out to him, which is a different
and more sensitive thing than a digest of a channel he already allowlisted — and `/voice`, the
only thing that uses them, already holds the write token. The read token gets `403`, and a test
asserts it. **None of them is an MCP tool**, and a test holds that line too: text a tool can
return is text that enters a model's context.

**No read-scope credential writes anything durable.** That is the wider rule the line above is one
case of: every durable write — a turn, a read mark, a cached summary — needs the write scope.
`/summary` is the interesting one, because it is readable at read scope and yet has something to
file: a read token is served from the cache when there is a hit and its answer is never written
back. `/inbox` is the single durable READ a read token may make, because how far the owner has
read is the thing the voice agent has to be able to say out loud.

**`/api/v1/inbox` carries `read_state_notice` on every answer**, including the mutating ones,
saying that this read state is gent-talk's own and is synchronised with Discord in neither
direction. See **Durable state** above.

**Every message this API renders carries `author_id` next to `author`** — in the scrollback, in
one-message reads, in `resolve` results, in the posted message echoed back by `reply`, and as a
field on every digest entry. That is what makes a real Discord mention possible: `@coding_agent`
typed as words notifies nobody, and only `<@1532416065114607829>` does. Bots have ids too, and
they are included, because addressing another coding agent is a legitimate reply.

**A page says that it is a page.** `GET .../page` answers with `returned`, `has_more`, and —
when there is more — `next_before` or `next_since`, whichever continues the walk it was asked
for. Two modes, and they cannot be mixed: `before=<id>` steps backwards from a cursor, and
`since=`/`until=` (ISO-8601, start inclusive, end exclusive) jumps to a period. A page tops out at
99 rather than 100 on purpose: the server fetches one more than it returns and drops it, which is
the only way to answer "is there more?" exactly instead of guessing from a full window.

Ranges work because a Discord snowflake carries its own creation time in its top 42 bits, so an
instant converts straight into a cursor. A numeric offset would have to be synthesised and would
drift as messages arrive, so there is deliberately no `offset=`.

**A count is bounded and says when it is a floor.** Discord publishes no message count for a guild
text channel, so `GET .../count` walks backwards a hundred at a time until the channel runs out or
`discord.max_count_scan` stops it, and sets `at_least` when the ceiling was what stopped it.
"At least 500" is the honest answer; a confident total would be either slow or wrong. This is the
defect the count tool exists for: asked how many messages a channel held, an agent answered
**100** — the size of the page it had been handed.

**Every message also carries two times, and only one of them is meant to be spoken.**
`spoken_time` is the instant already converted into `server.timezone` and labelled with it —
`09:51:25 EDT` — and it is what a voice agent reads out, verbatim, with no conversion of its own.
`timestamp` is the exact ISO-8601 instant Discord reported, unrounded, and it is what anything
computing with a time must use. The conversion happens once, in `src/ops.rs`, so the phone and the
voice agent cannot disagree about when something was said. This exists because handing an
assistant `13:51:25+00:00` and letting it do the arithmetic produced *"thirteen fifty-one Eastern
Time"* — the right clock with the wrong label, when nine fifty-one was the answer.

There is deliberately **no user-lookup tool**, and the reason is recorded in `src/model.rs`
beside the field. First, the id arrives ATTACHED to the message being replied to, so there is no
lookup step, nothing to search for, and nothing for a model to hallucinate — a wrong snowflake
pings a stranger. Second, it bounds the capability: the only ids that ever exist here belong to
people and bots that have actually spoken in an allowlisted channel, whereas a lookup tool would
let the voice agent ping anyone in the server.

`POST .../reply` authorizes exactly the users the text itself mentions
(`allowed_mentions: {parse: [], users: [...]}`). `@everyone`, `@here`, and role mentions remain
unreachable, and an empty `allowed_mentions` — which is what this used to send — would have made
`<@…>` render as a mention that silently notified nobody.

`resolve` takes `{"query": "...", "limit": 50, "max_alternatives": 3}` and answers with `best`
(the full message, or `null`), `alternatives`, and `ambiguous`. **A query that matches nothing
returns `best: null`** rather than the newest message wearing a confident label; a voice interface
that guesses is worse than one that says it did not find anything.

## Resuming a conversation across a hang-up

The vendor cannot resume a conversation once the socket closes — the initiation message and the
signed-URL endpoint both take an `agent_id` and neither accepts a `conversation_id`. So this
server does not resume anything. What it does instead is hand a NEW call a written record of the
old one, so the agent can carry on from it.

**That is a reconstruction, and every part of this is arranged so it cannot read as more than
one.** `replay.enabled` is off by default; when it is on, `GET /api/v1/conversations/{id}/replay`
renders the stored transcript and the page sends it on the new socket.

### The states, which are that many different sentences

The failure to avoid is not that this breaks; it is that it half-works and the screen keeps saying
it worked. So the clause under the large control is derived rather than written:

| State | What the screen says |
|---|---|
| off, here or on the server | "the agent starts fresh" |
| armed, whole record sent | "the earlier conversation is replayed" |
| armed, budget dropped some | "the earlier conversation is replayed **in part**" |
| the fetch failed | "the agent starts fresh — the earlier conversation could not be read" |
| nothing was said in it | "the agent starts fresh — there was nothing to replay" |
| the budget dropped ALL of it | "the agent starts fresh — the earlier conversation was too long to replay" |

**The last two are the same `included: 0` and they mean opposite things.** Both send nothing, and
only `dropped` tells them apart. "There was nothing to replay" said about a conversation that was
merely too large is a claim about what the reader said earlier, made with no basis for it — so the
page reads `dropped` as well as `included`, and Settings names the two budget settings that would
fix it rather than leaving the reader to conclude their conversation vanished.

The end-of-call seam changes with it. Every version of it asserted that the agent below the line
has never seen anything above it, and that stops being true the moment resuming is armed; with it
on, the seam says the new call will be read the lines above and that this is a reconstruction, not
the same conversation. A failed replay never aborts the call — it degrades to a fresh one and says
which it was.

### The budget, and why it is stated rather than discovered

A transcript grows without bound, the payload is billed per call, and the model's window is
finite. The rule: keep the most recent turns until either `replay.max_chars` or `replay.max_turns`
is reached, drop **oldest first**, and report how many were dropped. When anything was dropped the
agent is told so inside the payload as well — otherwise it insists the user never mentioned
something the user definitely did.

`0` is refused rather than read as "no limit": a budget of nothing is a replay that is always empty
while the interface says resuming is on, which is a feature that is silently doing nothing.

### The transcript is untrusted text

A stored turn is the owner's own speech AND whatever channel text the agent read aloud to him,
which is third-party Discord text. Every turn is neutralized and the whole record is fenced by
`src/untrusted.rs`; the preamble sits OUTSIDE the fence, because it is this server speaking and
the point of the fence is that nothing inside it is. A turn that forges the fence is defused and
the tampering stays visible.

### Privacy

**Every new call re-sends earlier conversation content to ElevenLabs**, including Discord text
written by other people that the agent read out. That is why it is off by default, why the
Settings screen says it in those words, and why it deserves an explicit decision if any of that
content is sensitive.

### The one check that can answer the real question

Whether the vendor puts a `contextual_update` sent immediately after the initiation message in
context for the **first agent turn** is a property of the platform. Nothing in this repository can
settle it:

```sh
scripts/run.sh --smoke-agent --replay-check
```

Three conversations, so three times the cost. A states a nonce and its turns are recorded through
this server's own transcript API, exactly as `/voice` records them; B opens WITH that record and
must return the nonce; **C is the control** — same question, no record — and must NOT be able to
answer, or the run proved the agent is fluent rather than that it remembers. All three outcomes
are reported separately, and "the vendor did not honour it" has an exit code of its own (21)
rather than being folded into a generic failure. It refuses, billing nothing, when this server has
no durable store.

**Until that check comes back green on a deployment, the interface must not claim a call was
resumed there.** `replay.transport = "client_data"` exists so the other path can be measured too:
it carries the text on the initiation message under `dynamic_variables`, which depends on the
agent's dashboard security settings permitting overrides and fails SILENTLY when they do not.

## Live push

Everything else here is pulled: a question arrives, a channel is read, an answer goes back. This
is the exception, and it exists for two things — the channel view on `/voice` should update as
messages arrive rather than when something happens to poll, and a reply that lands mid-conversation
should be able to reach the voice agent on its own instead of waiting to be asked about.

Two design decisions sit under it. Both were made before any of it was built, and both are written
here and in the module documentation of `src/live.rs` rather than left to be inferred from a diff.

### Decision one: ingestion is bounded POLLING, not a Discord Gateway connection

Discord's real-time mechanism is the Gateway. **This server does not use it.** With
`discord.live_poll_seconds` set, a background task reads each allowlisted channel on that interval
and publishes whatever is newer than a per-channel snowflake cursor.

That is deliberately the less capable option:

* **A Gateway is three new things at once.** A WebSocket client for Discord (there is one for
  ElevenLabs and none for Discord), a heartbeat / resume / session-invalidate state machine, and
  the privileged `MESSAGE_CONTENT` intent — which the application has to be granted, and without
  which every message arrives with an empty body.
* **Polling reuses what is already tested.** `DiscordClient::fetch_page`, `sort_oldest_first`
  and `MessageId::numeric` are the whole mechanism, and all three already have tests — including
  the string-versus-numeric snowflake ordering trap, which is exactly the bug a hand-rolled cursor
  reintroduces, and the `before`/`after` asymmetry, which is the other one.
* **The fake can genuinely fail.** `FakeDiscord::fail_next` and its unknown-channel refusal mean
  the poll loop's failure path is exercised by the suite rather than reasoned about.

And one reason that is about sequencing rather than design: **the Discord layer here has still
never run against live Discord.** Making first contact and introducing a stateful always-connected
client in the same change is two untested things at once, and when it misbehaves there is no way
to tell which one is wrong.

**The Gateway is the upgrade path, behind this same seam.** Everything above `src/live.rs` sees a
`LiveHub` — a channel-keyed publish/subscribe with a bounded replay tail. A Gateway implementation
would publish into exactly that and delete the poll loop; no route, no page and no test above the
hub would change.

Three rules in the loop are correctness, not tuning:

* **The first tick seeds the cursor and publishes nothing.** Otherwise the first poll after a
  restart republishes the whole recent window, and a page that attaches a second later is shown
  existing history labelled as newly arrived — and relays it into a paid conversation as news.
* **A failed fetch does not advance the cursor** past anything it did not publish. Whatever
  arrived during the outage is published once it recovers, rather than skipped.
* **Every tick after the first walks FORWARD from its cursor**, with Discord's `after`, rather
  than re-reading the most recent `default_fetch_limit` messages. Re-reading the newest window and
  then moving the cursor to the newest of them drops everything in between the moment a channel
  produces more than one window between two ticks — silently: no gap event, no warning, and a page
  and an agent simply missing lines. Walking forward cannot lose them.

**And a bound on catching up, because "read until caught up, whatever it costs" is a request storm
against a shared rate limit — and obeying `Retry-After` buys time, not quota.** One tick walks at
most four pages per channel — 200 messages at the default — and stops. Nothing is skipped: the cursor only ever moves past
what was actually published, so the next tick continues where this one stopped. Being stopped by
that ceiling is logged **once**, at WARN, on the way into it, and the recovery is logged too;
delivery is running late and that is worth seeing, but it is lateness, not loss.

**A failing channel backs off** by doubling up to sixteen intervals, and that is pinned by a test
that measures how long the loop actually waited against a healthy control. It has to be measured,
because deleting the line that applies the backoff changes nothing else observable: the loop still
fetches, still publishes, still recovers, still logs.

**And when the failure is a rate limit, the backoff does not undercut it.** A 429 during a tick is
first waited out by the client itself (see "Discord rate limits"); what reaches this loop is a
limit that did not clear inside that budget, still carrying Discord's outstanding `retry_after`.
The loop waits the larger of its own doubling and that number, so the two are one mechanism at two
scales rather than two rival opinions about when to come back. That is measured too: after a
single failure the doubling alone says twenty seconds, which is not long enough for a limit that
asked for forty-five.

### Decision two: the PAGE keeps the ElevenLabs conversation socket

The server relays nothing to the vendor. It mints a signed URL and that is the end of its
involvement; the browser holds the conversation. Moving that socket server-side would turn
gent-talk into an always-connected, **billed** conversation holder whose cost accrues while nobody
is in the car, and would put third-party channel text on a vendor socket no human is looking at.

**The cost of that decision, stated plainly: contextual updates reach the agent only while the
page is open.** Close the tab and the channel keeps moving, this server keeps ingesting, and the
agent hears nothing until somebody opens `/voice` again and asks.

### The stream

`GET /api/v1/channels/{id}/stream` is Server-Sent Events, and is an ordinary read in every other
respect: same bearer token, same read scope, same channel allowlist. A channel outside the
allowlist gets `404 unknown_channel`, never a `200` that streams nothing — on a stream those two
are indistinguishable forever.

* Every event carries `id: <message id>` and `event: message`, and a payload of
  `{message, self_posted, replayed, untrusted_content_notice}`. The notice is the same one
  `/messages` carries: a pushed message is third-party text exactly as a fetched one is.
* **`replayed` says whether the event came out of the tail or off the wire just now**, because on
  the wire those are otherwise identical. Every attach that carries no `Last-Event-ID` — a fresh
  sign-in, a channel change, the reconnect after an `event: reset` — is handed the whole tail, and
  a page that could not tell would announce up to two hundred old messages to a live conversation
  as news. See "What the page does with it".
* **Reconnection.** The browser sends `Last-Event-ID`; the server replays only what came after it,
  from a bounded tail of the last 200 published messages per channel. The receiver is attached
  *before* the tail is read, under one lock, so nothing can slip between the two.
* **Falling behind.** A subscriber further behind than the broadcast buffer gets one
  `event: reset` and the stream **ends**. The page re-reads through the paged route rather than
  resuming short: a silent gap is the one outcome this design must not produce.
* `Cache-Control: no-store`, and `X-Accel-Buffering: no` so an nginx-shaped proxy does not buffer
  the response into nothing.
* **In the access log it leaves exactly one line, at attach, with `millis=0`.** The middleware
  returns as soon as the status is known, which for a streaming body is before the first event.
  A stream held open for an hour still logs zero. That is pinned by a test so nobody reads it as
  an instant request.

### What the page does with it

`/voice` reads the stream with `fetch` and a reader over the response body, **not** `EventSource`:
`EventSource` cannot carry an `Authorization` header, and the usual workaround puts a bearer
credential in a query string — in every proxy log and in the browser's own history, which is the
exact thing `/api/v1/signed-url`'s `no-store` and the page's `redact()` exist to prevent.

Arriving messages are rendered by the same element construction as fetched ones, de-duplicated
against the rows actually on screen, and — only if the reader has turned it on — relayed into a
live conversation as a `contextual_update`. Four guards on that relay, all load-bearing:

* **A replay tail is never relayed.** The stream opens with what the server already published, up
  to 200 messages, and every attach without a `Last-Event-ID` gets all of it. Those rows belong on
  screen; announcing them says "a message was just posted" about text that may be hours old, in a
  burst, into a conversation billed by the minute — the same "existing history labelled as new"
  failure the seeding tick exists to prevent, arriving through the other door. The cost of the
  rule, stated: a message that lands while the page is between streams reaches the list but not
  the agent, exactly as one that lands while the tab is shut does.
* **Self-posted messages are never relayed.** `ops::reply` posts as the bot and the poller reads
  that post back; relaying it would have the agent hear its own answer as news and answer it, in a
  loop that bills. The ids this server posted are recorded and travel with the message as
  `self_posted`. It is deliberately not an author comparison — this server does not know its own
  bot's user id, because `HttpDiscordClient` never calls `/users/@me`.
* **A Settings toggle, off by default.** Every channel message reaching a live conversation is
  both a cost and an interruption.
* **A live socket.** There is nowhere to send it otherwise, and queuing it for the next call would
  deliver stale news at the start of a conversation about something else.

## Signed conversation URLs

Turning on **Enable Authentication** on an ElevenLabs agent closes the public `talk-to` link. From
then on a conversation can only be started from a **signed URL**, minted from ElevenLabs' API with
an account key and good for about fifteen minutes. `GET /api/v1/signed-url` does that minting, and
`/voice` is a plain page — no build step, no CDN, no vendor bundle — that fetches one and opens the
conversation over a WebSocket.

**Failures are shown in the page, not in the console.** This route answers with a real taxonomy —
`503 elevenlabs_not_configured` names the exact setting that is unset, `502 elevenlabs_error`
carries the vendor's status and message, including things only the vendor knows, such as an API
key that lacks the `convai_write` permission — and `/voice` renders that sentence verbatim in an
alert panel. Nobody should have to open dev tools to learn a key is missing a scope. The page
redacts its own stored token before displaying anything, so an error body that echoes the request
back cannot put a credential on screen; `tests/js/voice_page.test.mjs` asserts both. Saving the
token changes the **button** — "Saving…", then "Saved ✓" — and the line beneath states what is
stored right now, because a success banner over an unchanged button leaves it ambiguous whether
the click even registered.

```jsonc
// GET /api/v1/signed-url, with the WRITE-scope bearer token
{
  "signed_url": "wss://api.elevenlabs.io/v1/convai/conversation?agent_id=…&token=…",
  "agent_id": "agent_…",
  "valid_for_seconds": 900
}
```

Four things about it are deliberate.

**It needs the WRITE scope**, even though it reads nothing. What it hands back is a working
conversation with your agent, and that agent has a credential of its own: if you gave it the write
token, then whoever holds a signed URL can ask it to post in your name. The gate on this route has
to be at least as strong as the strongest thing the conversation can do. An unauthenticated
`/signed-url` would be strictly worse than the public link that enabling authentication just
closed.

**The account API key never leaves the server.** It travels to ElevenLabs in an `xi-api-key`
*header*, never in a URL, and every error built from an ElevenLabs response body is redacted
first — an upstream is free to quote a credential back in its own error text, and that text ends up
in a log and in an API response. There is a test that asserts the key appears in no response body
on any path this route can take, including the vendor-rejection path.

**There is no unsigned fallback.** Three failures are distinguished, because they have three
different fixes:

| What happened | Status | `error` |
|---|---|---|
| `elevenlabs.api_key` or `elevenlabs.agent_id` is not set | `503` | `elevenlabs_not_configured`, naming the setting |
| ElevenLabs refused us, or is unreachable | `502` | `elevenlabs_error`, carrying the vendor status |
| The caller has no token, or only the read token | `401` / `403` | `unauthenticated` / `forbidden` |

**The answer is `no-store`.** The minted URL is itself a bearer credential for the next fifteen
minutes.

The `/voice` page expects the agent's output audio format to be **PCM** (`pcm_16000` is the
default). It reads the format out of the conversation's initiation metadata and says so plainly if
it is something it cannot decode, rather than playing noise.

### The status line is a message, not a fixture

It used to be a permanent grid row at the top of the dock: a strip of a phone screen held on every
frame, for a line that is blank most of the time. The governing rule of the layout is that the
majority of the space belongs to content, and a row reserved for something that is usually not
there is the clearest possible violation of it.

It now ships hidden, appears when there is something to say, and takes itself away after six
seconds — or immediately, if you tap its dismiss control. **Dismissal is a hide, not an erase**:
what was last said stays readable to anything that asks.

**It is an overlay, and that is the one exception to this page's "nothing is positioned" rule.** A
row that appears and disappears resizes `#scroll-area`, and resizing the scrolling element moves
the transcript under the reader's thumb — the exact defect `#47 scrollback-stability` exists to
remove. Floating it costs no reflow. The CSS says so, at length, so it does not get "cleaned up".

**A message that goes away can hide something nobody saw**, so nothing that must survive is kept
only here: a failure is in the `#error` panel until it is fixed, a close code is in the connection
detail on the settings screen, a conversation boundary is a seam in the transcript, and live /
muted / idle is carried by the controls themselves. What is left on the strip is a thing that was
true a moment ago.

Two things moved out of it rather than away:

- **The channel's own summary** — how much is loaded, and whether that is all of it — is now an
  entry at the head of the channel view, in the same seam idiom the transcript uses for a
  conversation boundary. It is a standing fact about what you are looking at, and it scrolls off as
  you read rather than expiring after six seconds.
- **The "paste your token" instruction** is in the sign-in screen's own body. Firing a toast at
  page load to say it was a message over the screen that was already asking.

`02-idle` is the screenshot that proves the issue is fixed: the idle phone, at rest, with nothing
holding a strip. `08-clear-armed` is its positive control — the same page with something to say,
saying it — because a page that had simply deleted the line would satisfy the first alone.

### The messages you have not dealt with yet

`#50 todo-view`. A long backlog of assistant messages is a to-do list in practice: some want a
reply, some only want reading, and nothing on this page could tell them apart. **To do** is a
sub-toggle of the channel view — the same list filtered, not a third tab — and it reads a
different route rather than filtering the rows already on screen, so there is one definition of
"dealt with" and it lives on the server.

**Read/unread is OURS, and this is the screen that has to say so.** `#61 unread-status` settled it:
Discord shares no read state with a bot, so there is no ack to send and no field to read. Marking
something dealt with here does **not** mark it read in Discord, and reading it in the Discord app
does **not** clear it here. The page says that on screen — quoting `store::INBOX_NOTICE` from the
server's own answer rather than restating it, so the two cannot drift — because the alternative is
that the owner meets the divergence first and has to guess which of the two is broken.

**The store is single-tenant.** Every mark is "the owner's", with no column saying whose. Sharing
one deployment between two operators would silently merge their inboxes; that is not a
configuration this decision survives, and it would have to be revisited rather than worked around.

Three acts, each reachable from a control:

- **Done**, on the row. One message, and the answer names it, which is what makes the undo exact.
- **Clear the backlog**, above the list. Bulk and destructive, so it says how many it is about to
  take, takes two taps — the same armed idiom the dock's Clear control uses — and lapses back to
  safe after a few seconds. It sends `{through: <newest id>, limit: <the window it read with>}`
  and the **boundary is included**: the row you gave up on is part of what you gave up on, and
  that is tested at the boundary. The `limit` is not decoration: the server resolves `through`
  against a window of its own, so a boundary sent without one is resolved against the server's
  default and clears messages that were never displayed. Clearing work the owner never saw is the
  worst failure this view has, so the window the list was READ with rides along with the act.
- **Undo**, as a chip, carrying the exact set the server said it cleared. Not "the last N", which
  is a different set by the time it is pressed. A bulk clear that also claimed messages you had
  dealt with *earlier* would resurrect them on undo, so the server reports only what it really
  changed.

**The channel does not stop while you work through it.** A message can arrive on the live stream
or come in on the 45-second poll while the filter is on, and it is by definition not dealt with —
so it is one more thing to do, and everything that *describes* the list moves with it in the same
moment the row appears: the head of the list, the count on **Clear the backlog**, and the Newest
chip when the row landed off screen. A view that drew the row while its own head still said
"nothing left to deal with" would be worse than one that missed it — a false statement with the
counter-example visible directly beneath it — and a bulk control that undercounted would clear
more than it offered to.

Two consequences worth stating. The walk back is **not** offered over a filtered list: the cursor
belongs to the unfiltered channel, and stepping back with it would prepend messages you have
already dealt with above ones you have not. And the head of the list says "**3 of 4**" rather than
"3 messages", for the same reason `#62 message-count-accuracy` exists — the second reads as the
size of the channel.

**What is not here yet**, said plainly rather than left to be discovered:

- **The gesture layer.** `#50` asks for a swipe to dismiss and a press-and-hold to declare
  bankruptcy. Disambiguating horizontal from vertical intent on the one list this page scrolls is
  a change of its own; the acts land first, each reachable by a control a keyboard can also get
  to, so a gesture becomes a second way in rather than the only way.
- **Detecting a reply.** `#50` also wants a message to leave the list when it is answered through
  Discord's reply affordance. That is *derived* state and it needs a reply reference on the
  server's `Message`, which is a wire-format change touching every struct literal that builds one.
  Until it lands, "dealt with" here is always **declared** — which is also why nothing yet has to
  decide what happens when derived and declared disagree.

### A long message can be read as a summary of itself

`#49 cached-summaries` landed as a **server half with no caller**: the endpoint answered, the store
cached, the policy-versioned key invalidated correctly, a startup sweep collected what a changed
policy orphaned — and nothing on any screen ever asked for a summary. A cache nobody spends is a
cost with no benefit.

**Summaries** is a chip over the channel list, beside "Collapse all", and it is a **mode** rather
than a per-row control. Collapsing to a prefix stays the default and costs nothing; a reader who
turns the mode on sees each folded row's opening three lines replaced by one line about the
message, with the message itself still one tap away behind **More**.

Four properties, all of them about cost rather than about appearance, and all of them tested:

- **"Long enough to summarise" is `COLLAPSE_OVER_CHARS`** — the same sentence `#47
  scrollback-stability` folds by, in the same function. A short message has no fold control and is
  never sent anywhere. A second threshold here would be a second definition of short, and the two
  would drift.
- **One request per message, ever.** The record is written BEFORE the await, not in the response
  handler, so a hundred scroll events over one row cost one request even while the first answer is
  still in flight — which is the case that matters, because a phone on a slow connection is where
  the events pile up. That is tested against a fixture whose answers are HELD, not merely against
  one that replies instantly: with an immediate reply, a record written on completion looks
  exactly like a record written before the request. The forty-five-second background poll, which
  replaces every row on screen, also buys nothing a second time.
- **Only what you are looking at.** Rows are asked about as they come near the viewport, so
  nothing is spent on messages nobody scrolls to.
- **The page names the summariser it actually got**, quoting the server's own answer at the head
  of the view. The shipped backend truncates — no model, no comprehension — and showing its output
  without saying so would be claiming a reading nobody paid for. `web/voice.js` contains no
  backend name of its own, which is asserted, because a constant here would outlive the deployment
  it described.

Two answers are deliberately different. `below_threshold` is the **server's** own, stricter
threshold saying the message is short enough to read as it is: settled, never asked again, and the
row keeps its opening lines. Anything else without usable text is a **failure**: the row falls back
the same way, the strip says so once, the error panel is not raised for one row out of fifty — and
leaving the mode and re-entering it retries it, because a failure is not a verdict.

Entering the mode changes the height of every row on screen at once and adds a sentence above the
list, which is the mutation browser scroll anchoring does not cover. It goes through the anchor
helper `#47` built, and the page suite proves it with a negative control built from a copy of
`web/voice.js` with that call removed.

### The channel view walks back, one server-cursored step at a time

`GET /api/v1/channels/{id}/page` landed with **#53 stepped-retrieval** and had **no web caller at
all**: the page read `/messages`, which is a window, so the oldest message on screen was simply the
end of what this interface could ever show — with nothing on the screen saying so.

The channel read now uses the cursored route. Reaching the top of the list takes the next step by
itself, which is the gesture people already have; **Older messages**, above the list, is the same
action as a control — it is what *says* more exists, what a keyboard can reach, and what reports a
step in flight. It is absent once the walk has reached the beginning, following the same rule Hang
up does.

Older messages arrive **above** the viewport, which is exactly the mutation a browser's own scroll
anchoring does not cover — the same case as collapsing a message the reader has already scrolled
past. It therefore goes through the anchor helper `#47 scrollback-stability` built, not a second
mechanism beside it.

Two things this makes possible that were not:

- **A re-read no longer discards the walk.** The newest page replaces only what it covers; rows
  older than it survive when the server says `has_more`, and are dropped when it says the page *is*
  the whole channel. Without that, a background poll would delete the history out from under
  somebody reading it, every forty-five seconds.
- **The count can finally be true.** `#62 message-count-accuracy` says a number is the channel's
  own only when the server reports the set is complete. Walking back to the beginning is the first
  way this page has ever been able to reach that state, and the summary then says
  "13 messages from lead team" instead of "the most recent messages".

Snowflakes are compared as **strings**, length first. A Discord id is nineteen digits, `Number()`
rounds it, and two adjacent ids come out as the same float — an ordering test written with `<` on
numbers quietly answers false for a list that is perfectly ordered.

The throwaway server the screenshot harness starts sets `discord.max_fetch_limit = 8` on purpose:
`--fake-discord` seeds about a dozen messages, so at the default ceiling the channel arrives in one
read and none of this is exercised by any picture.

### Choosing the channel is a control, not a line of the scrollback

The picker used to be a row at the **top of the scrolling region**, above the log. That is the
whole of the defect the owner reported: the control for choosing *what you are reading* scrolled
away the moment you read anything, and getting back to it meant scrolling a channel to its
beginning.

**Paging made it worse, and that half was ours.** Once reaching the top loaded another page,
scrolling toward the picker prepended history above it — so it receded as you approached, and on a
channel of any size it could not be reached by scrolling at all. `#83 channel-selector-in-bar`.

So it moved onto the **control bar**, which `#58 control-bar` built as a packing container for
exactly this, and the fix is placement rather than tuning: with the picker out of the scrolling
element, the automatic step back is free to keep working as designed and nothing here touches
`OLDER_TRIGGER_PX`.

It is the **first** member of the pack, and that is the reachability argument rather than a taste
one — the pack scrolls sideways when it is full, so the member at its left edge is the one on
screen without scrolling anything. It may shrink but never grow (`flex: 0 1 auto` against the
`flex: 1` every `select` inherits from `style.css`), because a member that grew would take the
width the pack holds for the buttons beside it.

**Every member of the pack says which views it belongs on**, in one table (`PACK_VIEWS` in
web/voice.js) rather than in a branch. The rule is where the member's effect lands: the picker
names what the channel view is reading, and Type, Sumry and Blockers all go out through
`sendUserMessage`, which draws the line into the **transcript** and needs a live call — so on the
channel view they are three controls whose whole result appears on a screen you are not looking
at. A member with no line in the table is hidden everywhere and the page suite names it; there is
no default, because a default is how a control ends up on a view nobody chose for it.

That is a layout decision as much as a semantic one, and it is **measured**: the bar is one strip
on a 375px phone, six controls do not fit on it, and the pack scrolls — so a control pushed past
the pack's right edge is exactly as unreachable as the picker used to be. "The bar fits on the
owner's 375px phone" costs whatever `renderControlBar` really leaves visible against widths read
out of web/voice.css, on each view and in text mode, and the same test proves it is not vacuous by
showing that all six together overflow by about 55px.

**Refresh stays where it is.** It is a re-read of what is already on screen rather than a choice
about what to read, it keeps your place, and a keyboard reaches it wherever the list is scrolled
to.

The screenshot state `30-channel-picker-in-bar` is the check written for the acceptance criterion,
because "unreachable" is ultimately a measurement a layout engine makes: it walks the channel back
several pages, parks mid-history, and asserts the picker is inside a 375px viewport on all four
sides and did not move by so much as a pixel across the whole walk. **It has not been captured.**
No host this has run on has had Playwright or Chromium, so what is checked today is the offline
arithmetic above and `--self-test`, which confirms the scene is *pinned* to those claims without
rendering a frame. The same is true of `31-pull-to-refresh-armed`. Both are written and neither is
evidence yet; the first run with a browser is what turns them into any.

### What to call a channel

`#39 channel-alias`. A channel arrives with two names and neither is sayable: the snowflake
`1532416065114607829`, and whatever `label` the operator wrote in the configuration file — which
is often something like `build noise`, chosen when there was one channel and nobody was addressing
it out loud. The motivation is the next feature rather than this one: with several channels
configured, *"ask the build channel"* has to resolve to something, and a short name the owner
chose is the only candidate.

So the owner can **give a channel a name of his own, in this app**, from Settings. The alias wins
wherever the configured label showed; the label is the fallback, and clearing the alias returns to
it. `ChannelInfo::display_name` is the one place that rule lives, so the picker, the head of the
channel and the digest header cannot come to disagree.

**Both served pages, not just the one with the editor.** The alias is set from `/voice` Settings,
but it is shown at `/` too — both channel pickers there and the channel-name header — because two
pages of one deployment showing two names for one channel is precisely the disagreement
`display_name` exists to prevent. In the browser the rule is a function called `channelName`, and
it exists twice: `/` and `/voice` are separate assets with no build step between them, so they
cannot share a module. What holds them together is a test in `src/http/api.rs` over the bytes of
**both** files, asserting that each defines the rule and that every place either one names a
channel goes through it. That guard is not new — it was widened after `#52 operator-timezone` and
`#62 message-count-accuracy` were each fixed on one page and carried across afterwards — and
`#39` is the third time the same trap was walked into. `tests/js/app_page.test.mjs` is the
behavioural half for `/`, alongside the long-standing suite for `/voice`.

**It is ours, and it is local.** It lives in the `#64 storage-backend` store beside the read
state, exactly as `#50 todo-view`'s "dealt with" marks do — the same posture, and for the same
reason. It is never sent to Discord, renames nothing there, and nobody outside this deployment
sees it. Every answer that sets or clears one carries a standing statement saying so, and `/voice`
shows *that* sentence rather than one of its own.

**The voice agent hears it too.** `list_channels`, the digest header and the tool manifest all
name the channel the way the owner does, so the name he says out loud is the name the model was
given. That is a read of local state on the way out; nothing about it is a write to Discord.

**The agent cannot choose it, and the reason is not the scope.** A hosted voice agent is routinely
given the write token — `post_reply` needs it — so gating a rename behind the write scope would
not keep a model out, and this README is not going to claim it does. What keeps it out is that
**there is no tool**: the MCP manifest offers none at either scope, and `dispatch` refuses any name
it cannot find there. `tests/alias.rs` asserts both, with the write token, and the page suite
asserts the browser half — nothing arriving on the conversation socket reaches the alias route.

Retention deliberately does not reach this table. There is at most one row per configured channel,
so it cannot grow with use, and an age bound would be worse than absent: it would put the
configured label back months later with nothing on screen saying why. `DELETE /api/v1/storage`
still takes it, because "erase everything" has to mean everything.

Out of scope, and stated so nobody reads more into it: the agent changing an alias, renaming
anything in Discord, and multi-channel addressing itself — which this only prepares for.

### Pulling the channel down, and the other end of the same container

The owner found the channel hours out of date: *"especially when I swipe up on this view and it
shows me something very stale."* The staleness itself was already fixed — the view re-reads on
entry and polls every forty-five seconds while it is up — and that covers being stale *and
waiting*. It gives no way to say **refresh, now**. `#68 pull-to-refresh`.

**The contention is the whole design question**, because pull-down-at-the-top and
load-older-on-scroll-up are the two ends of one scrolling element. The rule is two sentences, and
it has to be two, because the obvious one-sentence version leaves the gesture unreachable on
exactly the channels that need it:

1. **A finger on the glass suspends the automatic step back.** Deferred, never dropped — it is
   taken the moment the finger lifts, so `#65 scrollback-paging` stays automatic rather than
   becoming a button.
2. **The pull is measured from where the list ran out**, not from where the finger landed. An
   overscroll begins at the edge, so the pixels spent reaching the top are scrolling and only the
   ones after it are a pull.

So the reader has one continuous motion for each meaning: drag up through the history and the list
scrolls (lift within `OLDER_TRIGGER_PX` of the top and the deferred step fires, and the walk back
continues); drag down until the list runs out and keep going, and the extra travel past the edge
is the pull. `overscroll-behavior: contain` is what leaves that overscroll to this page instead of
letting the browser reload the whole application under it.

**The one-sentence version was wrong, and it shipped.** Judging the gesture by the scroll position
at `touchstart` reads well — "the list is already at its top, so this drag is an overscroll" — and
it describes a state a reader on a paged channel can never be in: arriving at the top fires the
step back, the anchored prepend puts them at a positive offset again, and so `scrollTop === 0`
never holds until the whole history has been walked. The feature existed only on channels short
enough not to need it. Rule 2 is what replaces that reading while keeping what it got right: a
flick started a little below the top runs the list out within a frame, so the first `touchmove`
already reports zero — anchoring at the edge means that flick has travelled nothing yet, where
judging it from where the finger *is* would turn ordinary scrolling into a refresh.

Two more rules keep the features out of each other's way: a step already **in flight** refuses to
arm a pull (its answer is about to prepend content above the viewport), and an **armed** pull drops
the deferred step rather than taking it on release — the reader asked for the newest end, and
answering with more history is answering the opposite question.

A pull is also **one finger going down**. A drag whose sideways travel exceeds its vertical travel
is refused for the rest of the touch (the platform's own edge-swipe carries tens of pixels of
downward drift), and a second finger landing mid-gesture refuses it too rather than re-reading
`touches[0]` and silently restarting the measurement from wherever finger one has reached.

**It says what it is doing, and then what it found.** *Pull to refresh* while dragging, *Release to
refresh* once past the threshold — before the finger lifts, so the gesture can still be abandoned —
and *Refreshing…* while the read is out. Then one line: *refreshed — something new had arrived*, or
*refreshed — nothing new since the last read*. A refresh that finds nothing must not look like a
refresh that never happened.

**A pull goes to the newest message; the button keeps your place.** That difference is deliberate:
a pull is made *at the top of the history* asking for what is new, and keeping your place there
means staying at the oldest thing loaded. **Refresh** (`#refresh-discord`) is pressed from wherever
you are reading, so it keeps you there — and it stays, because a desktop has no gesture and a
keyboard reaches a button.

### One channel row at a time, but only where a pointer exists

A long channel is a wall of blocks, and Discord picks out the row under the pointer because a mouse
gives it something to pick out **with**. That is the whole justification and also the whole limit:
on a touch screen a `:hover` rule fires on tap and then stays lit until you tap something else — a
row that looks selected when nothing is.

So the tint sits behind `@media (hover: hover) and (pointer: fine)`, which asks the two questions
that matter: can this device hover at all, and is its pointer precise. Not a width query — a
touchscreen laptop is wide and must not get it — and not a user-agent string. It is deliberately a
*second* query rather than being folded into the desktop regime above, because a narrow window with
a mouse should still pick out its rows.

The keyboard's equivalent, `:focus-within`, is **ungated**: a keyboard is not a pointer, and
somebody tabbing through the channel needs the same "you are here" on any device. The row's
actionable child is the reply control, and hovering or focusing the row picks that out too. It is
not *revealed* by hover the way Discord's row actions are — the standing rule on this page is that
a control which cannot act is absent, so a control that can act must not be invisible.

Contrast is the real deliverable here and no test can judge it, which is why `20-discord-hover`
sits beside `10-discord-view` in the screenshot set: the pair is the with-and-without evidence, and
it was reviewed in both schemes because a tint that reads on white can vanish on `#1d2026`.

### Replying from the page

`POST /api/v1/channels/{id}/reply` existed from the start and, until now, had exactly one caller:
the voice agent. So a reply typed by hand did not exist, and nothing could tell an answered message
from an unanswered one — Discord records a `message_reference` only for replies made through the
affordance.

Every raw message in the channel view now carries a **Reply** control, and it opens a screen of its
own rather than a box wedged under the list: the message being answered has to stay legible while
you write, and on a phone a text field beneath a scrolling list means the keyboard covers both.
Three decisions worth stating, because the issue left them open:

- **The posted reply is appended, not re-fetched.** The server hands back the `Message` Discord
  accepted, which is better evidence than a later read — and a refetch replaces every child of the
  log, which would destroy the anchor the reader's position is measured against.
- **Drafts are per message**, and mirrored to `localStorage` with the same read-after-write check
  the token uses. One global draft would silently hand what you wrote about one message to a reply
  to a different one.
- **A send that fails changes nothing.** The text stays in the box, the reason appears beside it,
  the reader stays on the screen, and — because posting is not the call — a live conversation is
  untouched.

Coming back from the reply screen lands on the same line you left, and it does that with the same
anchor-and-offset mechanism the fold control uses rather than a saved `scrollTop`: hiding an
element is allowed to reset its scroll position to zero, and a restore expressed as a delta against
an anchor is correct either way.

### Two ways a channel row can already have been dealt with, and only one of them hides it

`#84 reply-aware-dismissal` lands the two follow-ups `#50 todo-view` named for itself — the **swipe**, and the
**derived "replied" signal** that needed a reply reference on the server's `Message`.

`#50` filters the channel on **dismissal**: declared by the reader, recorded on the server, with an
undo and a bulk clear behind it. That stays exactly as it was, and it is still the only filter.
`#66` adds the other half:

| State | Where it comes from | What it does |
|---|---|---|
| **Dismissed** | *declared* by the reader, held on the server | removes the row from the To do list |
| **Replied** | *derived* — some **loaded** message points at this one | dims the row; never hides it |

**`#50` left one question open** — "nothing in this file has to decide what happens when derived and
declared disagree" — and landing the derived half is what forces the answer. **They never meet**,
because they drive different affordances. Declared decides what is in the *list*: dismissing is an
act with an undo behind it, so it may remove a row. Derived decides how a row is *drawn*: replying
dims, and that is all. An observation must not make messages disappear, and this observation admits
it is incomplete.

**That incompleteness is the reason for the split, not a caveat bolted onto it.** Discord records a
reply only on the *answering* message — there is no field at all on the message being answered — so
the server carries the raw pointer (`Message::reply_to`, deliberately not a `has_replies` boolean,
which could only ever have meant "within this fetch window" while being read as more) and the page
derives the rest from what is loaded. A reply further back than the reader has walked is invisible,
so a row can be dimmed *later* than it should have been, never earlier. Being late to dim costs
nothing; being late to **hide** would lose a message.

The derivation is **one pass over the whole list after every mutation**, never a decision taken when
a row is built. Rows arrive from three places, and "has this been answered" is a fact about the
*set*: an answer can arrive in the next poll, and a step backwards can reveal the question a loaded
answer belongs to.

**The swipe drives the same dismissal the Done button does** — one act, one record, one undo. That
ordering was `#50`'s condition for a gesture layer at all, and it still holds: the gesture is a
second way in, never the only one, and a horizontal *mouse* drag is left alone because that is how
a person selects the text of a message they want to quote.

Replying from the page closes the loop on the spot: the reply carries a `message_reference`, so what
it answers dims immediately rather than at the next forty-five-second re-read.

### Settings is controls; Help is the paragraphs that used to be in the way

The settings screen had become a document with switches buried in it. Every knob carried two or
three paragraphs explaining itself — all of them worth having, none of them worth reading again on
the way to a checkbox — and on a desktop browser the whole thing ran the full width of the window,
which puts a checkbox at one end of a metre of desk and its label at the other.

So it is two screens now. **Settings is controls and labels**: each group is a bordered card with at
most one short line of orientation and a `?`. **Help holds the prose**, and every `?` opens the
matching entry. Nothing was deleted, and that is the part that makes the split survivable — the
paragraph about the switch you are looking at is still exactly one tap away, which is the only
reason moving it is an improvement rather than a filing cabinet.

Three things follow from that, and each is asserted rather than trusted:

- **The link is checked in both directions.** A settings group that links to an entry which does not
  exist fails; so does an entry nothing links to. The script's `HELP_TOPICS` must equal the markup's
  `data-help` set, so a new group cannot ship with a `?` that does nothing.
- **No paragraph on the settings screen may exceed a budget.** That is the measurable form of the
  complaint, and without it the prose creeps back one sentence at a time.
- **The disclosures are still disclosures.** The tests that guard what resuming and the relay cost
  now assert the control and its summary on Settings *and* the full text in the matching Help
  entry *and* the `?` that reaches it. A test that only looked at Help would pass just as well if
  nothing linked to it.

Both screens are held to a **fixed 34rem column** — not `var(--reading-width)`. That variable is the
reader's choice about the transcript, and dragging the transcript wider has no business reflowing a
form; a form's comfortable width is a property of the form. The desktop block had deliberately
exempted these two screens ("they are forms rather than reading, and giving them a column is a
separate judgement nobody has made yet"); this is that judgement.

**Connection status moved to the end and gained a word.** It is a readout — what the conversation
is, what audio format it negotiated, why the last one closed — and it was standing at the top of the
screen, in the position a reader arrives at, when what they arrive wanting is a switch. "Connection"
also read as somewhere you configure one.

### A conversation can end three ways, and they are three different sentences

A WebSocket close is one event, and the page used to have two things to say about it. That is one
too few, and the missing one is the common case on a phone: **put the handset in your pocket
mid-call and iOS suspends the page**, the socket dies, and the page greeted the owner on his return
with a red panel saying the connection to the voice agent had FAILED. Nothing had failed. He
switched apps.

`socket.onclose` is now the only classifier, and it distinguishes:

| What happened | What the page says | Dot |
|---|---|---|
| The page was in the background when the socket died | "Paused — the app was in the background." The large control offers **Resume**. | `suspended` |
| Something really broke between this browser and ElevenLabs | The failure panel, unchanged, naming where the fault is | `error` |
| The call ended | What it already said, in words rather than a close code | `ended` |

`socket.onerror` no longer reports anything by itself: it fires *before* the close, and only the
close knows whether the page was hidden. It records the fact and arms a 250 ms timer that the close
cancels — so an error with no close following still reaches the screen, which is what that timer is
for.

**Nothing auto-reconnects.** Returning to a phone that has quietly reopened the microphone and
started a conversation nobody asked for is a worse outcome than the banner this replaces. Resume is
a tap, and it runs the ordinary start path — which mints a **fresh** signed URL every time, so the
"expired credential" case is closed by construction rather than by handling: this page has never
reused one. The clause under the button says so, because "Resume" is the honest word for what the
reader wants and a dishonest word for what the agent gets.

The suspension test is a **heuristic** and is documented as one: a close within 2.5 s of the page
coming back still counts as part of the suspension, because iOS commonly delivers it on the way
*back* rather than while hidden. Too wide a window would excuse a genuine drop moments after a tab
switch, which is why the page suite carries the negative control both ways — a visible failure must
still raise the banner, and a page with the suspension check removed must fail the suspension test.

### The control bar is a container, and it sits where your thumb is

The gear and the Voice/Discord switch used to be the whole of the header. They are now a **control
bar**: one strip that can sit in either of two homes, and that is built to fill up.

**Bottom is the default, and "bottom" means directly above the big buttons** — not below them,
where small icons would sit under the thumb reaching for Talk, and not pinned to the viewport
floor, which is the corner curvature that ate the first word of the status line on the owner's
phone. Top — under the title, where it used to be — is a setting, kept in `localStorage` under
`gent-talk.voice.bar-placement` and offered in Settings. There is **one** bar and two mount points,
and `setPlacement()` moves the element between them; two copies would be two gears and two switches
for the page to keep in agreement, which is the defect rather than the layout.

**The bar is declared in the dock in `web/voice.html`**, not created from script, so the default
placement is a fact about the served markup: a page whose JavaScript died still shows the controls
where they belong.

**The gear is at the far left and the switch at the far right**, and that asymmetry is deliberate:
the gear is the control you least want to hit by accident, so it goes furthest from where a
right-handed thumb rests, and the switch — the one you actually flick — goes under it. What holds
them at the two ends is `#bar-pack`, the empty container between them, which takes all the leftover
width. That is also what makes the bar **pack**: buttons added to it scroll sideways rather than
wrapping onto a second row (which would spend the vertical space this is all about) or clipping the
switch off the right-hand edge of a 375px phone, where it could not be tapped at all. The switch is
the widest item in the bar, so that is arithmetic, not a worry.

**With the bar at the bottom the header holds nothing on the main screen, so it is hidden
outright** — the grid row collapses and the transcript grows into it. That is the real estate the
whole change is about; an empty 2.4rem strip across the top of a phone is exactly the rent this
project keeps refusing to pay. The header comes back the moment it has something to say: a title
and a way back on Settings or Reply, or the bar itself when the reader has chosen top.

Members hide **individually**, not the bar as a whole, which is what keeps the gear reachable from
the sign-in screen; the bar hides only when every member is hidden.

### The canned prompts, and the one whose wording had to be weakened

Two questions are worth a button because they are the ones actually asked, every time. **Sumry**
asks the voice agent to summarize; **Blockers** asks it to make the *coding agent* report. They go
out through the same `sendUserMessage` a typed turn does, so the sentence lands in the transcript
as the reader's own words, and a tap with no call open reports itself instead of doing nothing.

They differ in **weight**, and the difference is drawn rather than documented: Blockers spends
coding-agent work on one tap, so it is coloured in the same warm tone an armed Clear uses. It
deliberately does **not** ask twice — the issue did not ask for a confirm step, so adding one is a
decision for somebody to make out loud rather than drift into, and the suite records its absence
so it is not mistaken for an oversight.

It is a **list**, not two cases: `CANNED_PROMPTS` in `web/voice.js`, one entry per button, and one
loop that restores each field, saves it, and wires its button. A third canned prompt is one entry
plus one pair of elements in the markup, and the suite asserts there is no third place — a button
id named twice in the script fails, because the second mention is the special case starting.

Each prompt is **editable in Settings** and kept in `localStorage`; emptying a field puts the
default back rather than leaving a button that looks live and does nothing. The defaults are
written into `.value` from script, never typed into the markup: text between the tags of a
`<textarea>` is its child text, not its value, so a default written in HTML would be invisible to
everything that reads the field.

**The Summary prompt is deliberately weaker than the one the issue asked for.** It was filed as
*"Summarize my unread messages from the coding agent since I last messaged them"*, and
`#61 unread-status` established that both halves of that scoping are impossible here. Discord gives
a bot no read state at all — it is a client concept, with no ack route and no read-state field. And
the obvious fallback, *since the owner last spoke*, is not computable either: the owner has no
identity in this server, his own replies are posted **as the bot**, the digest drops the
bot/human flag, and the only author signal is a display name anyone can set to anything. A button
whose text claims that scoping would produce confident, wrong summaries — the exact failure this
project already paid for once, when an agent invented a digest. So the shipped default asks for
what the data genuinely provides: *"Summarize the recent messages from the coding agent in this
channel."* The suite pins that it says "recent messages" and says nothing about unread or about
when anyone last spoke. Building the capability — an `owner_id` in config, `author_is_bot` carried
into the digest, `before`/`after` on the Discord client — is a real feature and a separate one; the
research is in `ai_docs/UNREAD_STATUS_20260819.md`. Blockers is unaffected, because it makes no
claim about read state.

### Typing is the other way to say something, on the same conversation

Speaking is not always available — a quiet room, a commit hash the transcriber keeps mangling, a
name it will never get right — so `/voice` has a text composer, and it is deliberately **not** a
second mode or a second connection. `{"type": "user_message", "text": "…"}` is a client event on
the conversation socket that is already open, documented by the vendor as processed exactly like
speech. A typed turn and a spoken turn are therefore the same thing to the conversation, they land
in the same transcript, and the agent answers either one out loud.

**It costs one small button, and only while it is in use.** Type is a toggle in the control bar:
pressing it CONVERTS the bar into a text field — the toggle stays, the gear and the switch get out
of the way, the field takes the width and a Send appears on the right — and pressing it again
converts the bar back. One control both enters and leaves the mode, and its own pressed state is
what says which mode you are in. A permanent text field would be a whole extra band of dock on a
375x667 phone, competing with the transcript on every frame — the rent "The status line is a
message, not a fixture" above just stopped paying. A half-typed message survives leaving the mode.

The toggle appearing to *slide left* is the gear ceasing to exist beside it, not an animation:
flex simply closes the gap. There is no second mechanism to keep in step, and the only transition
is the colour.

`#43 typed-input` first shipped this composer as a row of its own in the dock, which was the
honest way to build and test the send path before there was a bar to put it in. That row is
**deleted**, not left standing beside the new one — two text fields racing to be the one somebody
types in is worse than either.

**Composing pings the agent.** `{"type": "user_activity"}` is documented as resetting the turn
timeout without touching conversation content, and it is sent at most once every thirty seconds
while somebody is typing. That is the direct answer to the complaint that the agent grows impatient
during silence and starts asking whether anyone is still there: a person composing a message is
present, and this is how the connection is told so. The frame carries no content — the suite
asserts it is exactly `{"type":"user_activity"}` — so what is half-typed never leaves the browser.

**Two things about it are unverified, and are written down as unverified rather than as knowledge.**
Whether ElevenLabs echoes a typed `user_message` back as a `user_transcript` is not documented
either way. If it does, the sentence would appear twice — once because the page rendered it when it
was sent, and once when the echo arrived — so a typed turn is remembered for ten seconds and a
matching transcript inside that window is dropped. The window is a guess; settling it costs one
billed `scripts/run.sh --smoke-agent` run, whose `converse()` already sends `user_message`, and
**that run has not been made**. The second is the on-screen keyboard: the dock is a grid row of a
`100dvh` frame, so the composer should ride above an iOS keyboard, but Chromium under Playwright
has no soft keyboard and no screenshot in this repository can prove it.

**Sound silences the agent's voice by receiving the audio and throwing it away**, and this is the
decision `#43 typed-input` asks to have recorded, because the alternative — renegotiating into a
text-only response mode — is indistinguishable to the reader and very different on the wire. A
text-only mode is settled at initiation: the page sends `conversation_initiation_client_data` once,
in `socket.onopen`, and reads the output format once, out of the initiation metadata. Switching
mid-call therefore means closing the socket and opening a new one, and the vendor documents no way
to resume a conversation — so the agent behind the reconnect would remember nothing, which is
exactly the context Mute exists to preserve. Dropping frames is reversible in the time it takes to
set a boolean; a reconnect is not reversible at all. The only cost is downstream bandwidth on a
socket already carrying microphone audio upstream. The suite makes that decision *checkable*:
toggling Sound must leave exactly one initiation frame on the socket.

### Mute says so out loud

`#73 mute-is-invisible`, and it is the owner's own complaint: during a long mute the agent "gets
very annoying about asking 'Are you there?'", and turning the prompting down in the ElevenLabs
dashboard did not help.

**The cause is ours.** Mute withholds `user_audio_chunk` frames and does nothing else, which is the
right pause — the socket stays open, so the agent keeps its context. But it means that on the
vendor's side a muted caller and a caller who simply stopped talking are the *same bytes*, and
going quiet is exactly the condition that makes an agent check whether anyone is listening. No
dashboard setting can separate two cases that arrive identical.

So the page says it. Muting and unmuting each put one `contextual_update` client event on the
conversation socket that is already open. That is **not a new event type and not a new mechanism**:
the page already sends `conversation_initiation_client_data`, `pong`, `user_audio_chunk`,
`user_message` and `user_activity`, and it already sends `contextual_update` twice over — once
after the initiation frame for `#46 conversation-replay`, and once per relayed message for the
Discord relay. Mute is a third use of a frame that was already on the wire. It **cannot** be an MCP
tool: MCP here is request/response with the agent as the client, and this server issues no
`Mcp-Session-Id` and answers `GET`/`DELETE /mcp` with 405 precisely because it has nothing to push.
The conversation socket is the only door.

A mute engaged in the **connect window** — after the socket exists and before it opens, which is
when the control reads "Connecting…" — cannot be announced at the time, because there is nothing
open to announce it on. It is re-announced from `onopen`, or the whole call would run muted with
the agent never told: the same complaint, for the length of the conversation instead of a pause.

**Half of this is unverified, and it is the vendor's half.** `contextual_update` is believed to
inject context without consuming a turn — the same event `#46 conversation-replay` uses — but that
belief came from a recon plan rather than from the vendor's protocol reference. Two questions are
open: whether ElevenLabs accepts the frame, and whether an agent that reads it *holds* rather than
prompts. **One billed `scripts/run.sh --smoke-agent` conversation answers both** — mute for a
minute and listen — **and that run has not been made.** What is checked offline is our half:
`tests/js/voice_page.test.mjs` pins what the page puts on the wire, and `tests/elevenlabs_mock.rs`
sends the page's own sentence, read out of `web/voice.js`, to the loopback vendor and pins that it
is recognised as a contextual update mid-call, that its text reaches the agent's context, that no
turn is spent on it, and that the conversation still works afterwards.

**That last paragraph is a statement about our model, and it is only worth reading because the
model exists.** Until it did, `src/elevenlabs/mock/` had no `contextual_update` handling at all:
the frame fell into the catch-all for events the mock does not understand, so the test above passed
unchanged when the frame was renamed to `totally_made_up_event` — it pinned that unknown events are
ignored and nothing else. The mock now models the event, in one documented place, with a control
test that keeps an unrecognised event landing somewhere different. It is still our model of the
vendor's contract, not an observation of the vendor.
The fallback if the event does not exist is a short `user_message`, which is definitely in the
protocol and *does* consume a turn — the agent would say "understood, I'll wait" out loud, which is
more interruption than the prompting it replaces.

Pair it with the agent's system prompt: the starting prompt in
[`QUICKSTART.md`](QUICKSTART.md) now tells the agent, in prose, to hold — "skip your turn, say
nothing, and do not ask whether he is still there" — when it is told the microphone is muted. It
does **not** name the `skip_turn` tool, which is a separate native vendor feature described further
down; nothing here wires the two together.

**None of this makes a mute cheaper.** Billing continues while muted; the vendor discounts silent
periods but does not stop the meter. This makes a long mute quieter, not free.
### A typed conversation is a different conversation, not a call with the switches thrown

Because a text-only mode is settled *at initiation*, it can be **chosen** there — and that is what
**Type** does when it is pressed with nothing open. The owner's report is what this answers:
reaching a text interface used to mean starting a voice call, muting it, and silencing it. Two of
those three controls exist to manage a microphone he did not want open, and mute deliberately does
not close one — it withholds frames from a live capture graph so that unmuting keeps the agent's
context — so the phone showed the microphone as in use for a conversation that was being typed.

`start({chat: true})` therefore **skips the microphone entirely**: no `getUserMedia`, so no
permission prompt and no in-use indicator; no `AudioContext`; no capture graph; no playback. Talk
and Sound are *absent* rather than inert, because there is nothing for either to act on, and Hang
up becomes "End chat" and takes the space Talk would have had. The suite asserts the guarantee in
the only form that is actually a guarantee — `micRequests`, `tracks` and `processors` are all
empty — rather than asserting that something was muted afterwards.

The page **also** asks the vendor for a text-only response, as
`conversation_config_override.conversation.text_only`, and **does not rely on getting it**. That is
an override, and an agent whose dashboard forbids overrides ignores it silently. If audio arrives
anyway it is dropped — there is no `AudioContext` to play it with — and the fact is recorded once
in the connection details, in as many words: the override was refused, nothing is played, and the
microphone was never opened. The guarantee the page makes is the one it can keep on its own side of
the socket; the override is an optimisation on top of it, and the difference is stated rather than
blurred.

### The page has two compositions, and a capability query picks between them

The phone is the device this page is used on, so the phone layout is the one everything above
describes. On a wide screen with a pointing device it becomes something else: the transcript and
the channel are held to a **reading column** instead of filling the window, the dock follows that
column rather than spanning the desk, and a handle on the column's edge — draggable, and reachable
from the keyboard — sets how wide it is. The width is stored per browser, in characters, and is
clamped to 45–120 on the way in and on the way out, because storage is shared with everything else
on the origin and a hand-edited entry is a thing people do. The settings screen carries a slider
that drives exactly the same value, so the choice is reachable without a mouse.

**The regime is `@media (min-width: 900px) and (pointer: fine)`, and nothing else decides it.**
Not a user-agent string — a tablet with a trackpad and a phone in desktop mode both lie to one, and
the strings keep changing — and not `matchMedia` in the script either, because two places deciding
what a desktop is, is two places that can disagree. `web/voice.js` only ever sets a number; the
stylesheet decides whether that number means anything, which is why the same code path runs on a
phone and does nothing visible there. The page suite asserts both halves: that the rules really are
inside that query, and that no rule for a capped pane exists outside it.

Judging whether the result reads as a desktop application is not something a fixture with no layout
engine can do, which is why the screenshot harness gained a second desktop width and two states for
the column at each end of its range — see "Looking at the page" below.

## The MCP endpoint

`POST /mcp` is an MCP server speaking **Streamable HTTP**. A hosted voice agent — ElevenLabs, in
the first intended deployment — is the client; this process is the server.

**Streamable HTTP, not HTTP+SSE.** MCP's original remote transport was HTTP+SSE: a long-lived
`GET /sse` stream plus a separate `POST /messages`, correlated by a server-held session. It was
superseded by Streamable HTTP, which is a single endpoint. This server implements Streamable HTTP
only, and deliberately does not offer the legacy transport rather than offering it badly — the old
one needs a session table and an always-open stream per client, which is real state on a process
that is meant to be publicly reachable.

**Stateless.** No session id is issued and none is required. Every POST carries its own bearer
token and is answered on the spot, so there is no session table to leak or fixate, and a restart
cannot strand a client. The cost is that the server cannot initiate messages — it has none to
send — so `GET /mcp` and `DELETE /mcp`, which exist in the spec for exactly that, answer `405`
rather than holding open a stream that would never carry anything.

**Content negotiation.** A client that accepts `application/json` gets a JSON body. A client that
accepts only `text/event-stream` gets a one-event SSE response and the stream ends. Both are
tested.

| Method | Behaviour |
|---|---|
| `initialize` | Echoes the client's protocol revision when it is one of `2025-06-18`, `2025-03-26`, `2024-11-05`; otherwise answers with `2025-06-18`. |
| `notifications/initialized` | Accepted, `202`, no body. |
| `ping` | `{}` |
| `tools/list` | The tools this credential may use. |
| `tools/call` | Runs one tool. |
| anything else | `-32601` method not found. Resources, prompts, sampling and logging are not implemented, and say so. |

JSON-RPC batches are refused with `-32600`. The 2025-06-18 revision removed them, and a
half-succeeded batch has no coherent HTTP status — which is not a property a write-capable
endpoint should have.

### The seven tools

| Tool | Scope | Approval intent | What it does |
|---|---|---|---|
| `list_channels` | read | automatic | Names the configured channels and which are postable. |
| `digest_channel` | read | automatic | One speakable line per recent message. |
| `read_page` | read | automatic | One step of a walk: a page that says it is a page, plus the cursor for the next one. |
| `count_messages` | read | automatic | How many, up to a cost ceiling — "at least N" when the ceiling stops it. |
| `find_message` | read | automatic | Describe a message in your own words, get it back in full. |
| `read_message` | read | automatic | One known message by id. |
| `post_reply` | **write** | **requires approval** | Posts as the bot. |

They are deliberately thin. With a real model in the loop the model should compose, not consume
pre-chewed operations, so there is no "summarize and reply" tool and no batching helper.

`ask_agent` — the slow-path seam that answers HTTP 501 — is **not** offered over MCP, and cannot
be called by name either. A tool whose only possible outcome is an apology spends a model's turn
on nothing. It stays in the manifest at `GET /api/v1/agent-tools` as the record of the intended
shape.

**A read credential is not shown `post_reply` at all,** and is refused with HTTP `403` plus
JSON-RPC `-32001` if it calls it anyway. Hiding and enforcing are separate on purpose: hiding a
tool is never the thing that keeps it from running.

**Every message a tool renders carries its author's mention token**, written as
`[id | time | author <@author id>]`, so a model that wants to notify someone copies a working
token instead of assembling one — and can only ever mention someone who has posted in an
allowlisted channel. See "The API" above for why there is no lookup tool.

**Every read tool's text output is fenced.** Channel content comes back inside the
`src/untrusted.rs` fence, with the data-not-instructions notice attached, with any forged fence
marked (not deleted) and control characters stripped. That is what the model actually receives.

### Registering it with an ElevenLabs agent

**Before anything else, check one blocker.** MCP integrations are **unavailable on ElevenLabs
workspaces in Zero Retention Mode, and on HIPAA-enabled workspaces.** On those the custom-MCP-server
option is not offered at all, and this entire integration path is closed — there is no workaround
here. Check the workspace setting before spending an hour on the rest.

Create a fresh agent and keep ElevenLabs' hosted LLM; nothing in this project needs you to bring
your own model. Then add a custom MCP server integration:

| Field | Value |
|---|---|
| Server URL | `https://<your-tunnel-hostname>/mcp` — the path **must** be `/mcp` |
| Transport | **Streamable HTTP** |
| Authentication | header `Authorization`, value `Bearer <your gent-talk read token>` |
| Approval mode | **Fine-Grained Tool Approval** |

**Streamable HTTP, not SSE.** If the dashboard also offers SSE, do not pick it. HTTP+SSE was MCP's
original remote transport and was superseded by Streamable HTTP in protocol revision `2025-03-26`;
this server implements only the current one, so choosing SSE will simply fail to connect.

Then set per-tool approval. This table is the whole policy:

| Tool | Approval setting | Why |
|---|---|---|
| `list_channels` | **auto** — no approval | Names the configured channels. Touches Discord not at all. |
| `digest_channel` | **auto** — no approval | One spoken line per recent message. The main one. |
| `find_message` | **auto** — no approval | A description in your own words → that message, in full. |
| `read_message` | **auto** — no approval | One known message by id. |
| **`post_reply`** | **REQUIRE APPROVAL** | **Speaks in your name, in your channel.** |

That is what implements "reading is automatic while driving, posting asks first". If the agent
only shows four tools, you gave it the read token — which is correct, deliberate, and means
posting is off entirely rather than gated.

A starting system prompt for the agent — triage rather than transcription, which is the whole
reason a digest exists — is in [`QUICKSTART.md`](QUICKSTART.md) under step 5.

**Which token you give it decides the ceiling.** With the read token the agent physically cannot
post — `post_reply` is not even listed for it. With the write token it can, subject to the
approval prompt. The conservative first deployment is the read token.

**Say this plainly: the per-tool approval setting is enforced on ElevenLabs' side, and this server
cannot verify it.** Nothing here can tell whether the owner configured approval correctly, or
whether it was later changed. What this server enforces is the scope split and the allowlist:
a read token cannot post, and no token can reach a channel outside the configuration. Do not
mistake the approval prompt for a guarantee we implement.

Verify the public endpoint before pointing an agent at it — same script, public URL:

```sh
scripts/verify-deployment.sh --url https://<your-tunnel-hostname> --channel <snowflake>
```

The read token's `tools/list` must NOT contain `post_reply`; check 2 is the one that asserts it.
If it fails there, something is wrong with the deployment, not with the agent, and no amount of
dashboard configuration will fix it.

## Putting it on the public internet with a Cloudflare Tunnel

The server speaks plain HTTP and holds a bot token, so it must never be exposed directly. A
Cloudflare Tunnel gives it a public HTTPS hostname with **no inbound port open** on the host: the
`cloudflared` daemon dials out and Cloudflare terminates TLS.

You need a domain on Cloudflare; the hostname can be a subdomain of one you already have.

```sh
# once, on the host
cloudflared tunnel login                                     # browser; pick the zone
cloudflared tunnel create gent-talk                          # prints a UUID and a JSON path
cloudflared tunnel route dns gent-talk gent-talk.<your-domain>
```

`tunnel create` prints the two values the next file needs: the **tunnel UUID** and the path to the
**credentials JSON** it just wrote (normally `~/.cloudflared/<uuid>.json`).

`~/.cloudflared/config.yml`:

```yaml
tunnel: 6f2a1c30-1111-2222-3333-444455556666
credentials-file: /home/<you>/.cloudflared/6f2a1c30-1111-2222-3333-444455556666.json

ingress:
  - hostname: gent-talk.<your-domain>
    service: http://127.0.0.1:8080
  # Required, and must be last: cloudflared refuses to start without a catch-all rule.
  - service: http_status:404
```

`tunnel:` also accepts the tunnel's name, but the UUID is what `create` handed you and it cannot be
ambiguous.

```sh
cloudflared tunnel run gent-talk        # foreground first, so you can watch it connect
```

Verify the public hostname with the same deployment check you ran locally:

```sh
scripts/verify-deployment.sh --url https://gent-talk.<your-domain> --channel <snowflake>
```

Then install it so it survives a reboot:

```sh
sudo cloudflared service install
sudo systemctl status cloudflared
```

Bind the server to loopback when you do this — `GENT_TALK_BIND=127.0.0.1:8080` — so the only path
in is the tunnel. With Podman, publish to loopback: `-p 127.0.0.1:8080:8080`.

### Do not put Cloudflare Access in front of this

Access is the obvious next fence and it is the wrong one here. **Access expects a human with a
browser**: an unauthenticated request gets a login redirect. ElevenLabs calls this endpoint
machine-to-machine, with no browser and nobody to log in, so Access bounces every call — and the
failure shows up inside ElevenLabs as a vague connection error rather than as "there is an
identity gate in the way", which is an expensive hour.

If you want Access anyway it has to be a **service token**: create one under Zero Trust → Access →
Service Auth, write a policy allowing it on this hostname, and add both headers to the ElevenLabs
MCP server configuration alongside `Authorization`:

```
CF-Access-Client-Id:     <client id>.access
CF-Access-Client-Secret: <client secret>
```

**A tunnel is transport, not authorization.** It gives you TLS and it hides the host's address; it
does not decide who may call. This server's bearer tokens do that, and that is the part this
codebase tests.

## Where ElevenLabs attaches

From the related-work review, and recorded in `src/mcp/mod.rs` so it does not have to be rediscovered:

* ElevenLabs Agents connect to **remote MCP servers over SSE or streamable HTTP** — so this server
  is the MCP endpoint and the agent is the client.
* Auth is a **secret token or custom headers**, which is why this server's API is bearer-token
  based: the agent configuration carries the read token.
* There are **three approval modes**, and the useful one is **per-tool approval**, which maps onto
  the rule this project wants: **reading is automatic, posting asks first.**
* **Barge-in and `skip_turn` are native**, but neither one is pause. Barge-in interrupts the
  agent while it is *speaking*; `skip_turn` is the agent choosing to hold. In both cases the
  socket stays open and the microphone keeps streaming. There is no vendor pause primitive: the
  only transport-level action is closing the conversation. **Pause is ours** — muting stops
  uploading audio while keeping the socket and the agent's context (`web/voice.js`), and Sound off
  silences the agent's voice while its replies keep arriving as text.
  Two things follow, and both are the owner's observation rather than ours: **billing continues
  while muted** (a conversation is billed for being open, though the vendor discounts silent
  periods), and **the agent will start asking whether you are still there**, because a
  client-side mute is invisible to it — from the vendor's side, muted and "went quiet" are the
  same thing. See *Mute says so out loud* above for what the page does about the second one, and
  for the part of it that is still unverified. The first one has no fix: a muted call is a call.
* Caveats: MCP is unavailable on Zero Retention Mode **and HIPAA-enabled** workspaces — which
  would block this integration outright — channel text transits ElevenLabs, and conversation
  costs roughly $0.01/minute.

`GET /api/v1/agent-tools` serves the tool list with that policy attached, and a test asserts the
invariant that **every mutating tool requires approval and every read-only tool does not**. The
live MCP endpoint builds its `tools/list` answer from that same manifest, so the tools a model
sees and the tools this project documents cannot drift apart.

### Why this server speaks MCP itself, rather than wrapping something that already does

There is an existing Discord MCP server on hand: the `discord` plugin shipped in the Claude Code
plugin cache under Apache-2.0 (publisher not stated in its metadata — no `author`, `repository`,
or `homepage` field, and no marketplace manifest naming one). It is a real MCP server with
Discord fetch/reply/react/edit/download tools and a thought-through access-control layer. Three
options were weighed.

**Wrap it with a stdio-to-HTTP proxy.** `mcp-proxy` is the obvious candidate and it is ruled out
on its own documentation, on two counts. Its mode that exposes a local stdio server remotely is
**SSE only** — Streamable HTTP appears only in the opposite direction, as a client connecting to
a remote server. And it has **no inbound authentication**: its `--headers Authorization` flag
forwards a token *outbound* to a backend; there is no mechanism to *require* one from callers.
Wrapping as-is would therefore leave an unauthenticated public endpoint that can post to the
owner's Discord. That is disqualifying, and it means "wrapping" would really mean writing both a
Streamable HTTP server transport and the authentication into a third-party codebase — with the
security-critical half written by us anyway.

**Have this server proxy to the plugin as a stdio subprocess.** This keeps the plugin unmodified,
which is the appealing part. It was rejected because the policy would still have to be
re-implemented on top of it: the plugin's tools are keyed on *its* allowlist
(`~/.claude/channels/discord/access.json`, a pairing model designed for one human DMing an
assistant), not on this server's channel allowlist and read/write split, so every call would have
to be intercepted and rewritten — policy enforced on a surface we do not control, with two
allowlists free to drift apart. It also brings a Bun runtime, a supervised subprocess, a
persistent gateway connection and pairing state into a container that currently has one static
binary and no state. And the "upstream updates keep applying" benefit is thinner than it looks:
the plugin is a pinned `0.0.4` cache directory with no repository URL, updated by the Claude Code
plugin mechanism rather than by anything a container can track.

**Extend this server — chosen.** It already had every property the endpoint needs: bearer tokens,
a read/write scope split, a channel allowlist a valid token cannot escape, secret redaction, the
untrusted-content boundary, a Containerfile, and a passing test suite. What was missing was a
protocol, which is roughly 500 lines of JSON-RPC over a `Discord` trait that already does the two
things v0 needs. One process, one language, no subprocess to supervise, and the security
properties are enforced where they were already tested.

The plugin remains valuable as a **reference**, and was read as one. Its most useful specific:
**Message Content Intent must be enabled in the Discord Developer Portal**, or the bot sees empty
message content.

## Security

This is the part that deserves the care. The server holds a credential that can read and post to
the owner's channels, and it is intended to become publicly reachable — which makes it, not the
voice agent, the real security boundary of the whole design.

**What v0 does.**

* No unauthenticated route touches Discord. Every `/api/` route **and the `/mcp` endpoint**
  requires a bearer token, checked with a non-short-circuiting comparison. `/mcp` authenticates
  *before* it parses the body, and answers a bad or missing credential with a fixed
  `{"error":"unauthorized"}` — a test asserts that body names no tool, no channel, no protocol
  revision, and not even the service. An unauthenticated caller learns only that something is
  listening.
* **Reading and posting use different tokens.** The token you put on your phone and in the voice
  agent cannot post. The server refuses to start if the two tokens are equal or shorter than 24
  characters. Over MCP the read token is not even shown `post_reply` in `tools/list`, and is
  refused with `403` if it calls it by name anyway.
* **Configured channels are an allowlist.** A channel absent from the configuration answers 404
  even to the write token, however many channels the bot happens to be in. Each channel is
  additionally `writable` or not, defaulting to not. **Both front doors share one implementation**
  of that rule (`src/ops.rs`), and a test drives the same unconfigured snowflake through the REST
  route and the MCP tool and requires both to refuse — so the two cannot drift.
* **Posts cannot ping.** Every outgoing message sets `allowed_mentions: {parse: []}`, so a repeated
  `@everyone` in a summary cannot notify a server.
* Empty and over-long posts are refused before Discord sees them.
* Secrets are wrapped in a `Secret` type whose `Debug` prints `<redacted>`; a test asserts that
  neither it nor the whole `Config` leaks a token when formatted.
* The image runs as a non-root user and contains no configuration.
* Durable state is `0600` in a `0700` directory, is bounded by retention, and is off unless an
  absolute path is configured. See **Durable state** above.
* The web app never uses `innerHTML` and never renders markdown, so channel text cannot become
  markup or a tappable link.

**What v0 does NOT do — the honest list.**

* **No TLS.** Bearer tokens over plain HTTP are readable in transit. Terminate TLS in front of it
  before it is reachable from anywhere but a LAN.
* **No rate limiting and no brute-force delay.** A 33-byte random token is not guessable in
  practice, but nothing here slows an attacker down or tells you they tried.
* **No audit log of posts.** Posting is logged only as ordinary server output. If the bridge ever
  posts something you did not authorize, there is no separate record to reconstruct it from.
* **No token rotation.** Changing a token means restarting the server.
* **One token per scope, not per client.** Every reader shares one credential, so revoking one
  reader revokes all of them.
* **The write token is a full posting capability.** Anything holding it — the voice agent, a
  phone's local storage, a shell history, a container inspect — can post as the bot. Treat it as
  you would the bot token itself.
* **Approval is advisory in v0.** "Posting asks first" is enforced by the *agent platform's*
  per-tool approval setting, which this server describes but cannot verify. The server-side
  enforcement is the scope split and the allowlist, nothing more. Do not give the write token to
  anything you would not let post unsupervised.
* **No per-request replay protection, and no nonce.** A bearer token captured anywhere it is
  stored — the ElevenLabs agent configuration, a phone, a shell history, `podman inspect` — can
  be replayed until the token is changed, which means a restart.
* **The MCP endpoint has no request size limit of its own** beyond axum's defaults, and no
  concurrency cap. A caller holding a valid token can make the server fetch from Discord as fast
  as it will answer.
* **Channel text transits the agent platform.** Anything `digest_channel`, `find_message` or
  `read_message` returns is sent to ElevenLabs and to whatever model is behind it. The fencing
  constrains how it is *framed*, not where it goes.
* **Stored transcripts are not encrypted at rest.** Once `storage.path` is set, the owner's
  speech and the channel text the agent read aloud sit in the clear in a SQLite file that
  outlives the container. Anyone with host filesystem access can read it. Retention bounds how
  much accumulates; it does not protect what is there.
* **The bot's own permissions are the real ceiling.** Give the bot the narrowest Discord
  permissions that work, in the fewest channels. Server-side allowlisting is a second fence, not
  the first one.

**Prompt injection.** Discord message content is written by third parties — other people, other
teams' bots, anything that can post a webhook. It is **data, never instructions**. `src/untrusted.rs`
holds that boundary: content handed to a model is fenced, and any attempt to forge the fence is
neutralized *without deleting the hostile text* (deleting it would hide the attempt). Control
characters are stripped so escape sequences cannot smuggle framing. **Every MCP read tool returns
its channel text inside that fence** — the MCP path is the one where text reaches a model without
a human in between, so it is the path that most needs it, and an end-to-end test seeds a message
carrying a forged fence and a fake `SYSTEM:` directive and asserts the fence survives, the
forgery is marked, and the hostile text is still readable as data. What code cannot enforce is
that a model obeys the framing — which is precisely why posting is a separate, differently
credentialed capability instead of something a summary can trigger. Every read response also
carries an `untrusted_content_notice` field restating this to whatever consumes it.

## Development

```sh
cd gent-talk
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

# the deployment check, against an in-memory Discord — no bot token needed
cargo run -- --config gent-talk.toml --fake-discord &
scripts/verify-deployment.sh --url http://127.0.0.1:8080 --channel <a-configured-snowflake>
```

315 Rust tests plus a 159-test suite for the `/voice` page: unit tests beside each module,
end-to-end tests in `tests/api.rs` that drive the real router against the in-memory Discord, and
`tests/mcp.rs` doing the same for the MCP endpoint. `tests/elevenlabs_mock.rs` is the one place
the WHOLE chain runs — the real `HttpElevenLabsClient` mints against a loopback ElevenLabs
substitute, a real WebSocket is opened to it, and the conversation drives real MCP `tools/call`s
into the in-memory Discord, so the answer the agent speaks is made out of channel text that really
came back. The scenario that answers WITHOUT calling any tool — the 2026-08-19 production failure
— is one of its cases, offline and free. The page suite
(`tests/js/voice_page.test.mjs`) executes `web/voice.js` itself against a small strict DOM whose
element set is read out of `web/voice.html`, so a script reaching for an element the page does not
have fails there rather than silently at the roadside; `cargo test` runs it through
`tests/voice_page.rs`, which FAILS rather than skips when Node is absent. The fake is not a yes-man — it shares the real client's
request validation and ordering contract, and it records what was posted, so the API tests assert
the actual channel, content, and reply target that reached it. A handler that dropped a post,
posted to the wrong channel, or ignored the scope split fails those tests. The parts of the live
Discord client that cannot be exercised without a token — the URL, the `Bot ` authorization prefix,
the request body, and the payload-to-message mapping — are pure functions with their own tests.

**The page suite's scroll position is clamped, exactly as a browser clamps it**, and that is not
tidiness. `scrollTop` in a browser can never exceed `scrollHeight - clientHeight`; a fixture that
took `scrollTop = scrollHeight` at its word — which is how the page pins itself to the newest line
— modelled "at the newest message" as a whole viewport PAST the last row, so every question the
suite asks about following the newest line was being answered about a position that cannot exist.
It also left the anchoring assertion weaker than it reads: `getBoundingClientRect().top` is
`offsetTop - scrollTop` here and the restore is `scrollTop += top - before`, so "the reader did not
move" is algebraically forced for any height change at all — including a restore that scrolls past
the end of the content. The clamp refuses that, the anchoring test additionally requires the
resulting position to be strictly inside the range, and it carries a second negative control: the
same formula applied against a DIFFERENT element has to fail. `#74 scroll-test-strength`.

The safety tests were checked against deliberate breakage rather than assumed to have teeth. Four
mutations were applied one at a time and the suite was required to go red for each: removing the
bearer check in the MCP transport (3 failures), removing the channel allowlist in `src/ops.rs`
(3), removing the fence-forgery neutralization in `src/untrusted.rs` (5), and removing the
write-scope fence in the MCP dispatcher (1). All four were reverted; the suite is green.

### Looking at the page

Every test above drives behaviour. **Not one of them lays anything out**, so not one of them can
tell you the page looks right — the `/voice` suite says so about itself. That gap is not
theoretical: a single photograph of the owner's phone showed three defects the whole suite had
passed over, because a clipped paragraph and a control that reads as active on a dead call are
layout facts and a fixture with no layout engine has no opinion about them.

```sh
scripts/run.sh --screenshots
```

Photographs the `/voice` page in the thirty-four states that look different — signed out, idle, live
call, muted, the agent's voice silenced, just after a hang-up, the end-of-call seam with its
disclosure open, the clear control armed, settings, the Discord view, a long transcript parked
mid-scroll, that same list with one folded answer opened among the closed ones, the moment a turn
arrives while the reader is up in the history, the desktop reading column at each end of its
range, and the two connection outcomes that used to look identical — a call suspended by the
phone, and one that really failed — the reply screen with a short target and with one longer
than the frame, one channel row picked out under the pointer, a step further back through the
channel, the earlier conversation restored from the server after a reload, a turn that was typed
rather than spoken, the control bar in each of its two homes, that same bar converted into a
text field, the bar packed with every button it has, a channel message that arrived because the
SERVER pushed it rather than because the page asked, a resumed call whose reconstruction was
only partial and says so, the channel picker on the bar with the history walked back several
pages, the channel pulled down past the point where letting go refreshes it, the channel with its
collapsed rows summarised instead of clipped, the channel filtered to what has not been dealt
with, and the bulk clear saying how many it is about to take — at four viewports: a tall phone, a
short phone, a small laptop window and a maximised desktop. It prints the absolute path of every
image so an agent can open them directly.

**Some states exist on one class of device and are captured only there**, and the run says so by
name. The reading column at each end of its range is DESKTOP ONLY, because `@media (min-width:
900px) and (pointer: fine)` is what puts a column on the page at all — on a phone there is no
column, no handle and nothing to photograph. The armed pull-to-refresh is PHONES ONLY for the
mirror-image reason: `has_touch` is set per profile, and a desktop context cannot have the
gesture. A scene that ran on the wrong device and passed would be filed as evidence of an
interface that device does not have, which is why `Scene.profiles` exists and why `--self-test`
checks the restrictions are still on the scenes that need them.

**Dark is the default, and that is not a preference.** The owner's phone is dark, and the first run
of this harness captured nothing but light frames, because light is the browser-automation default
— so every image was real and every one was of a page he never sees. Contrast between the two
speakers is exactly what does not survive the swap. `--theme light` or `--theme both` when you want
the other one.

It costs nothing and needs no hardware. The conversation WebSocket is replaced before any page
script runs, and the mint request to `/api/v1/signed-url` is answered locally, so the live-call
states are reached without ElevenLabs being contacted. (That in-page wire fake is on its way out:
`src/elevenlabs/mock/` is a real loopback vendor — a real mint endpoint and a real socket — and
the cargo suite already uses it. Pointing this harness at it, so the photographed states depend on
a real handshake rather than a page-local stub, is the remaining half of #57 elevenlabs-mock.)
The microphone is Chromium's own fake capture device, so `getUserMedia`, the AudioContext and the
real capture graph all still run. It
stands up its own throwaway native server with `--fake-discord` on port 18091 and stops it again,
including on failure — it refuses port 8080 by name, because that is the live deployment.

The two ways a screenshot harness lies are both closed, and both were verified by mutation:

* **Thirteen pictures of the same idle screen.** Every state declares what must be true of the live
  page before the shutter opens, and each is pinned to its OWN marker — the post-call shot checks
  for the seam labelled "new conversation", the armed shot for the word the control changes to. A
  state whose expectations do not hold fails BY NAME and is not photographed approximately. A
  further control rejects any expectation still driving a selector the interface rework retired,
  because a stale selector does not fail loudly: `getElementById` returns null and the run blames
  the page.
* **A picture of a page that never rendered.** A white rectangle is a valid PNG. Every capture is
  decoded (in the standard library — no image dependency) and rejected if it is too small, too
  few bytes, under sixteen distinct colours, or more than 99.5% one flat colour. The check sits on
  the only function that writes a file, so there is no second path that saves an unjudged image.

Some expectations are load-bearing beyond "did we get here". The opened seam must land ABOVE the
dock — its first capture caught it unfolding underneath, where it is invisible — so that state
asserts the explanation's bounding box clears the dock's top edge. That assertion was verified end
to end by reintroducing the bug in the browser and confirming the run refused.

`12-collapsed-long-transcript` is the other one, and it is here because a LINE CLAMP CANNOT BE
TESTED ANYWHERE ELSE. The page suite can check that `-webkit-line-clamp: 3` is declared on
`.body.clamped` and that the page puts that class on the right element; whether three lines is
really three lines is a rendering fact, and this is the only thing in the repository that measures
it. It measures the SAME message closed and then open — comparing the first open message against
the first closed one compares two lengths rather than two states, and passed for the wrong reason
at desktop width until it was fixed.

**The walk is a chain, and where that is a lie it has been made false.** Most of these states are
one continuous session on purpose — `04-muted` is a picture of the call `03-live-call` started, and
rebuilding it would be a fiction. What is not allowed is a state that inherits something it does
not photograph: `13-jump-to-newest` used to show a "Collapse all" chip opened by the scene before
it, while its own description says "the chip appeared", singular — and neither it nor
`12-collapsed-long-transcript` could be run with `--only`, because "there is already a folded
transcript on screen" was a requirement nothing stated. The three transcript states now build the
list they photograph, through one shared act, and `--self-test` checks that they still do.
`#74 scroll-test-strength`.

`scripts/screenshots.py --self-test` runs 48 controls for those checks offline, with no browser and
no server; `scripts/test-run-sh.sh` runs them as part of its own suite. Screenshots are written to
the gitignored `debug/screenshots/` and are never committed. Playwright and its Chromium are the
only requirement, and a missing one fails by name with the install command.

The harness is opt-in and in no default suite. It rebuilds the binary first, because `web/` is
compiled into it with `include_str!` — without that you photograph the last build's markup and
believe it is today's.

## Layout

```text
src/config.rs         configuration, environment overrides, secret redaction
src/auth.rs           bearer tokens, read vs write scope
src/access.rs         the access log: what one line says, and what it must never say
src/model.rs          Message/Channel types, snowflake ordering
src/discord/          the DiscordClient trait, the live HTTP client, the in-memory fake
src/discord/ratelimit.rs  Retry-After, the X-RateLimit-* buckets, and the bounded wait-and-retry
src/summary.rs        extractive digest lines
src/retrieval.rs      semantic random access, behind the Ranker trait
src/untrusted.rs      the data-not-instructions boundary
src/ops.rs            the operations both front doors share: allowlist, fetch, transform
src/live.rs           inbound ingestion (bounded polling), the per-channel fan-out, and the SSE body
src/replay.rs         rebuilding continuity across a hang-up: the preamble, the budget, the fence
src/probe.rs          the startup channel reachability probe and its failure taxonomy
src/elevenlabs/       the SignedUrlProvider trait, the live client, the in-memory fake
src/elevenlabs/mock/  the loopback vendor: a real mint endpoint, a real WebSocket, a real MCP client
src/bin/mock_elevenlabs.rs     that mock as a process, for a browser or the smoke script
src/mcp/mod.rs        the tool manifest and per-tool approval policy
src/mcp/protocol.rs   JSON-RPC 2.0 and the MCP method set
src/mcp/transport.rs  the Streamable HTTP endpoint at /mcp
src/store/            the StateStore trait, the SQLite backend, the fake, the refusing one
src/summarize/        the Summarizer trait, the extractive backend, the counting fake, the cache key
src/agent_backend.rs  the slow-path seam
src/http/             router, handlers, and the access-log middleware
web/                  the phone app and the /voice page (plain HTML/CSS/JS, no framework, no build step)
tests/js/             the /voice page's own suite, run from cargo test via tests/voice_page.rs
scripts/verify-deployment.sh   the one-command deployment check, local or public
scripts/smoke-agent.py         the manual, billed check that the AGENT really calls us
scripts/screenshots.py         photographs /voice in every state, so an agent can SEE the page
QUICKSTART.md         the six-step setup path, start to first conversation
```

## Known gaps

Beyond the security list above:

* `resolve` only searches the window it just fetched (default 50, max 100 messages). Older messages
  are unreachable; there is no pagination and no store.
* Ranking is lexical. It matches words, not meaning, so a paraphrase with no shared words will miss.
  The `Ranker` trait is the replacement point.
* Summarization is extractive truncation, not a model. It shortens; it does not comprehend. The
  `Summarizer` trait, the policy-versioned cache and the `/summary` route are in place and
  tested; **no model backend is wired to them yet**, so `summaries.model` is recorded in the
  cache key and otherwise unused, and **no page requests a summary**. `#49 cached-summaries` is
  therefore the server half of that issue and not the whole of it: the seam, the key, the
  invalidation, the bounds and the sweep are done; the thing a person would see is not.
* No caching of channel content: every question is a fresh Discord fetch. Obeying `Retry-After`
  does not change that — it buys time, not quota, so a poll interval still spends one request per
  channel whether or not anybody is listening. That is why `discord.live_poll_seconds` still
  defaults to OFF and why an interval under five seconds is still refused outright rather than
  clamped. The summary cache is the one exception and it is a cache of DERIVED text, bounded and
  purgeable — see "Durable state".
* **`Retry-After` is handled; the status code and the absence of a queue are what is left.** A 429
  is parsed (body `retry_after` in fractional seconds, the `global` flag, and the `X-RateLimit-*`
  headers), waited out and retried, bounded at four attempts and thirty seconds; a bucket known to
  be empty is not spent; a global limit stops every channel rather than one. See "Discord rate
  limits". Two things are deliberately NOT done: **the exhausted case still reaches an API caller
  as HTTP 502**, not as a 429 with a `Retry-After` of its own, because the error-to-status mapping
  in `src/http/api.rs` was left alone; and **there is no queue** — a request that runs out of
  budget is over, not deferred to be sent later.
* **Live push has never run against live Discord either, and neither has the poller, and neither
  has the rate-limit handling.** The seeding rule, the cursor, the failure path, the SSE framing
  and every wait described under "Discord rate limits" are tested against the in-memory fake on a
  paused clock; the first real deployment with an interval set will be the first time any of it
  meets Discord's rate limiter.
* **Whether the vendor honours a replayed transcript is UNVERIFIED.** `#46 conversation-replay`
  is complete on this side — the budget, the fencing, the four honesty states and the transport
  switch are all tested — but no billed run has yet confirmed that ElevenLabs puts the payload in
  context for the first agent turn. `scripts/run.sh --smoke-agent --replay-check` is the check;
  it has never been run against a live agent. That is why `replay.enabled` defaults to false.
* **The replay budget is a guess.** 6000 characters and 40 turns were chosen, not measured. The
  payload is billed per call.
* **A contextual update reaches the agent only while `/voice` is open.** The conversation socket
  belongs to the browser, not to this server (see "Live push"), so closing the tab ends the relay
  even though the server keeps ingesting.
* The web app has no service worker and no offline mode.
* **The MCP endpoint has never been driven by a real ElevenLabs agent.** It is tested against the
  protocol as written and against `curl`; the registration steps above are from the vendor's
  documentation, not from a completed round trip.
* **The Discord layer has still never run against live Discord.** Adding MCP did not change that:
  both front doors go through the same untested-against-production client, and
  `scripts/verify-deployment.sh` has therefore only ever been exercised against the in-memory
  fake — in CI and by hand. Its checks are protocol-level and transport-level, so they apply
  unchanged to a real deployment, but the first real run will also be the script's first real run.
* **Signed URLs have never been minted from live ElevenLabs.** The endpoint, the header, and the
  response shape come from the vendor's current documentation; everything below that is tested
  against an in-memory ElevenLabs that can refuse, and against a loopback HTTP server that proves
  the key is sent as a header. The first live call will still be the first live call.
* The `/voice` page decodes PCM only. If an agent is configured to emit µ-law or MP3, the page says
  so and stops rather than playing noise — it does not transcode.
* **Press-and-hold on the talk control does not offer hang-up**, and `#43 typed-input` asks for it
  as a possibility rather than a requirement: it is worth doing only if a true *pause* exists, and
  whether one does is still open — see `#40`. Mute is what stands in for a pause today, and hang-up
  is its own control in the pane. Deferred deliberately, not dropped.
* **Whether a typed `user_message` is echoed back as a `user_transcript` has never been observed.**
  The `/voice` page assumes it may be and suppresses a matching transcript for ten seconds; one
  billed `--smoke-agent` run would settle it, and that run has not been made.
* **Whether ElevenLabs implements `contextual_update` at all has never been observed**, and two
  features now depend on it: `#46 conversation-replay`'s default transport, and `#73
  mute-is-invisible`'s announcement that a mute is deliberate. The page's half is tested offline
  against the mock vendor, which *models* the event — the model being this repository's belief
  about the contract, written where a test can state it, and not evidence of anything. Whether a
  real agent accepts the frame, and whether reading it actually stops it asking "are you there?",
  takes one billed `--smoke-agent` run that has not been made.
* The `/voice` page captures the microphone through a `ScriptProcessorNode`, which is deprecated
  (though universally supported). Moving it to an `AudioWorklet` would mean a second asset for no
  behavioural gain today.
* The legacy HTTP+SSE MCP transport is not implemented. A client that cannot do Streamable HTTP
  cannot connect.
* No MCP resources, prompts, sampling, or logging capability — tools only.
* No `Mcp-Session-Id`, therefore no server-initiated notifications and no `tools/list_changed`.
  A configuration change is picked up by restarting, and the client re-lists on its next connect.
* This crate is intentionally outside the repository's Rust workspace and is not part of
  `make check` / `make test`; it has its own CI workflow.
* **Discord's read state is still unreachable, and always will be.** Discord shares it with
  clients, not with bots — no ack route, no read-state field on the channel object, no
  `read_state` in the gateway READY payload. The evidence and the code anchors are in
  `ai_docs/UNREAD_STATUS_20260819.md`. What `#48 transcript-storage` added is a *different*
  record: gent-talk's own read marks, set through `/api/v1/channels/{id}/read` and never
  synchronised with Discord in either direction. Nothing yet **uses** them — no digest is
  scoped by one, and the `/voice` page does not set one — so "since I last read" is still not
  a question this server answers. The entity exists; the feature built on it does not.

## License

MIT, as with the rest of the repository.
