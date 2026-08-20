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
| 429 | Rate-limited, so readability was **not** established | Wait and restart. |
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
| Retention age | `storage.retain_days` | — | default `30`; `0` means no age limit, and it bounds cached summaries too |
| Summary threshold | `summaries.threshold_chars` | — | below this nothing is summarised, default `400` |
| Summary width | `summaries.target_chars` | — | default `160` |
| Summary context | `summaries.context_messages` | — | preceding messages shown as context, default `3` |
| Summary model | `summaries.model` | `GENT_TALK_SUMMARY_MODEL` | unset means the extractive backend |
| Config file path | — | `GENT_TALK_CONFIG` | or `--config` |

Environment wins over file. An empty environment variable is treated as unset, so a runtime that
renders unset variables as `""` cannot blank out a configured value. An unknown key in the file is
an error, not a shrug — a typo'd section name should not silently disable a setting.

## Durable state

Almost nothing in this server is remembered. A channel is never cached as a channel: every
question is a fresh Discord fetch, and it stays that way. There is exactly one store,
`src/store/`, and it holds three things:

* the `/voice` **conversation transcript** — what the owner said and what the agent said back;
* **read marks** — how far the owner has been shown each channel; and
* **cached summaries** — one short line per long message, filed under the policy that produced
  it. This is the one entry NOT authored by this server: with the shipped extractive backend a
  summary is literally the opening of somebody else's message, so it is a second at-rest copy of
  third-party text and is treated as one everywhere below.

The first two are the owner's own record. The third is a derived cache, bounded by the same
retention as the rest, erased by the same purge, and never filled by a read-scope token.

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

The age bound is the only thing that collects an **orphan**: a cached summary whose message was
edited or deleted upstream is unreachable by key, is filed under the current policy version, and
nothing will ever announce that it went. The startup sweep cannot help — it deletes by policy
version, and an orphan is under the live one.

### How an operator purges it

Three ways, all of which erase everything — transcripts, read marks and cached summaries alike:

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
| GET | `/api/v1/conversations` | **write** | stored `/voice` transcripts, most recent first |
| DELETE | `/api/v1/conversations` | **write** | erase every stored transcript |
| GET | `/api/v1/conversations/{id}` | **write** | one stored transcript, oldest turn first |
| DELETE | `/api/v1/conversations/{id}` | **write** | erase one stored transcript |
| POST | `/api/v1/conversations/{id}/turns` | **write** | record one turn |
| GET | `/api/v1/inbox` | read | how far this server thinks each channel has been read |
| POST | `/api/v1/channels/{id}/read` | **write** | move this server's read mark forward |
| DELETE | `/api/v1/channels/{id}/read` | **write** | drop this server's read mark |
| DELETE | `/api/v1/storage` | **write** | erase EVERYTHING durable: transcripts, read marks, cached summaries |
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

### Typing is the other way to say something, on the same conversation

Speaking is not always available — a quiet room, a commit hash the transcriber keeps mangling, a
name it will never get right — so `/voice` has a text composer, and it is deliberately **not** a
second mode or a second connection. `{"type": "user_message", "text": "…"}` is a client event on
the conversation socket that is already open, documented by the vendor as processed exactly like
speech. A typed turn and a spoken turn are therefore the same thing to the conversation, they land
in the same transcript, and the agent answers either one out loud.

**It is collapsed by default**, which is what makes it affordable. A permanent text field is a
fourth band of dock on a 375x667 phone, competing with the transcript on every frame — the rent
"The status line is a message, not a fixture" above just stopped paying. What stands on the screen
is one small button; the field appears when it is asked for, and a half-typed message survives
closing it again.

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
  same thing.
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

Photographs the `/voice` page in the twenty-five states that look different — signed out, idle, live
call, muted, the agent's voice silenced, just after a hang-up, the end-of-call seam with its
disclosure open, the clear control armed, settings, the Discord view, a long transcript parked
mid-scroll, that same list with one folded answer opened among the closed ones, the moment a turn
arrives while the reader is up in the history, the desktop reading column at each end of its
range, and the two connection outcomes that used to look identical — a call suspended by the
phone, and one that really failed — the reply screen with a short target and with one longer
than the frame, one channel row picked out under the pointer, a step further back through the
channel, the earlier conversation restored from the server after a reload, and the text composer
open during a live call with a turn that was typed rather than spoken, and the control bar in
each of its two homes — at four viewports: a tall phone, a short phone, a small laptop window and
a maximised desktop. It prints the absolute path of every image so an agent can open them directly.

The last two states are DESKTOP ONLY, and say so in the run: `@media (min-width: 900px) and
(pointer: fine)` is what puts the reading column on the page at all, so on a phone there is no
column, no handle, and nothing to photograph. A scene that ran there anyway and passed would be
filed as evidence of a layout the phone does not have, which is why `Scene.profiles` exists.

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

`scripts/screenshots.py --self-test` runs 44 controls for those checks offline, with no browser and
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
src/summary.rs        extractive digest lines
src/retrieval.rs      semantic random access, behind the Ranker trait
src/untrusted.rs      the data-not-instructions boundary
src/ops.rs            the operations both front doors share: allowlist, fetch, transform
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
* No caching of channel content: every question is a fresh Discord fetch, and Discord's rate
  limits are not handled. The summary cache is the one exception and it is a cache of DERIVED
  text, bounded and purgeable — see "Durable state".
* No `Retry-After` handling; a 429 from Discord surfaces as HTTP 502.
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
