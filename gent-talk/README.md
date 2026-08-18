# gent-talk

A small Rust web server that lets you talk to a voice agent about your coding agents' Discord
channels — from a phone, while driving — and, with explicit approval, post a reply back.

It is a **bridge, not an agent host**. It holds a Discord bot token, answers questions about
channels over an authenticated HTTP API, and serves a phone web app. It is deliberately **not**
co-located with the coding agents: it needs no access to any development workspace, and nothing in
it is tied to the machine it currently runs on. It starts in Podman on a laptop and is expected to
move to a small cloud host unchanged.

The design decision behind it, stated plainly: composing hosted products would also work (see
`../reviews/voice-agent-bridge-related-work.md`, which recommended exactly that), but a server is
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

## Status: what works, what is stubbed

| Piece | State |
|---|---|
| HTTP server, routing, JSON API | **works** |
| Config file + environment overrides, with validation | **works** |
| Bearer-token auth, read scope vs write scope | **works** |
| Discord read + post behind a trait | **written, unit-tested; never run against live Discord** |
| In-memory Discord for tests and `--fake-discord` | **works** |
| Digest / summarization | **works**, extractive and deterministic (no model call) |
| Semantic random access (`resolve`) | **works**, lexical ranking behind a `Ranker` trait |
| Web app: text tab, digest, find-a-message, local speech | **works** |
| **MCP over Streamable HTTP at `/mcp`** | **works.** Bearer-authenticated, stateless, five tools, tested end to end. Never yet driven by a real ElevenLabs agent. |
| ElevenLabs voice agent | **not verified.** The endpoint an agent needs exists and answers; no ElevenLabs agent has been pointed at it. The web app mounts the hosted widget only if an agent id is configured. |
| Slow path (ask a coding agent for detail) | **seam only.** The route exists and answers HTTP 501 with an explanation. |
| TLS | **not here.** Terminate in front (Caddy, nginx, a tunnel, or a cloud load balancer). A Cloudflare Tunnel recipe is below. |

Honest summary: everything except the two vendor-facing halves — live Discord and ElevenLabs — is
implemented and tested. Those two are exactly the parts that cannot be tested without credentials.

## What you must supply

Nothing is committed and nothing is hardcoded. You need:

1. **A second Discord bot**, separate from whatever your coding agents use, invited to the channels
   you want to reach, with `View Channel`, `Read Message History`, and — only if you want to post —
   `Send Messages`. Its token goes in `discord.bot_token` / `GENT_TALK_DISCORD_BOT_TOKEN`.
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

Against real Discord:

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

Check it, and confirm the credential is actually required — on both front doors:

```sh
curl -s localhost:8080/healthz
curl -s -o /dev/null -w '%{http_code}\n' localhost:8080/api/v1/channels          # expect 401
curl -s -H "Authorization: Bearer $GENT_TALK_READ_TOKEN" localhost:8080/api/v1/channels

# the MCP endpoint: unauthenticated is 401, and the body is deliberately uninformative
curl -s -o /dev/null -w '%{http_code}\n' -X POST localhost:8080/mcp \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'                             # expect 401

curl -s -X POST localhost:8080/mcp \
  -H "authorization: Bearer $GENT_TALK_READ_TOKEN" \
  -H 'content-type: application/json' -H 'accept: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'                             # no post_reply
```

The image runs as a non-root user and contains no configuration; a container started with no
credentials fails immediately rather than coming up with defaults.

## Configuration

| Setting | File | Environment | Notes |
|---|---|---|---|
| Bind address | `server.bind` | `GENT_TALK_BIND` | `0.0.0.0:8080` in a container |
| Public URL | `server.public_base_url` | `GENT_TALK_PUBLIC_BASE_URL` | informational |
| Discord bot token | `discord.bot_token` | `GENT_TALK_DISCORD_BOT_TOKEN` | **secret** |
| Read token | `auth.read_token` | `GENT_TALK_READ_TOKEN` | **secret**, ≥ 24 chars |
| Write token | `auth.write_token` | `GENT_TALK_WRITE_TOKEN` | **secret**, ≥ 24 chars, must differ |
| Channels | `[[channels]]` | `GENT_TALK_CHANNELS` | `id:label:rw` / `id:label:ro`, comma separated |
| ElevenLabs agent id | `elevenlabs.agent_id` | `GENT_TALK_ELEVENLABS_AGENT_ID` | public |
| ElevenLabs API key | `elevenlabs.api_key` | `GENT_TALK_ELEVENLABS_API_KEY` | **secret** |
| Config file path | — | `GENT_TALK_CONFIG` | or `--config` |

Environment wins over file. An empty environment variable is treated as unset, so a runtime that
renders unset variables as `""` cannot blank out a configured value. An unknown key in the file is
an error, not a shrug — a typo'd section name should not silently disable a setting.

## The API

All `/api/` routes require `Authorization: Bearer <token>`. `/healthz` and the static web app do
not, and neither reveals anything about the configuration.

| Method | Path | Scope | Purpose |
|---|---|---|---|
| GET | `/healthz` | none | liveness |
| GET | `/api/v1/channels` | read | configured channels |
| GET | `/api/v1/client-config` | read | what the web app needs at startup |
| GET | `/api/v1/agent-tools` | read | the voice agent's tool manifest and approval policy |
| GET | `/api/v1/channels/{id}/messages?limit=` | read | full scrollback, oldest first |
| GET | `/api/v1/channels/{id}/messages/{message_id}` | read | one message in full |
| GET | `/api/v1/channels/{id}/digest?limit=&width=` | read | one speakable line per message |
| POST | `/api/v1/channels/{id}/resolve` | read | **semantic random access** |
| POST | `/api/v1/channels/{id}/reply` | **write** | post as the bot |
| POST | `/api/v1/channels/{id}/ask` | **write** | slow path — answers 501 in v0 |
| POST | `/mcp` | read, or **write** per tool | MCP over Streamable HTTP — see below |
| GET/DELETE | `/mcp` | none | `405`; this endpoint is stateless and has nothing to push |

`resolve` takes `{"query": "...", "limit": 50, "max_alternatives": 3}` and answers with `best`
(the full message, or `null`), `alternatives`, and `ambiguous`. **A query that matches nothing
returns `best: null`** rather than the newest message wearing a confident label; a voice interface
that guesses is worse than one that says it did not find anything.

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

### The five tools

| Tool | Scope | Approval intent | What it does |
|---|---|---|---|
| `list_channels` | read | automatic | Names the configured channels and which are postable. |
| `digest_channel` | read | automatic | One speakable line per recent message. |
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

**Every read tool's text output is fenced.** Channel content comes back inside the
`src/untrusted.rs` fence, with the data-not-instructions notice attached, with any forged fence
marked (not deleted) and control characters stripped. That is what the model actually receives.

### Registering it with an ElevenLabs agent

In the ElevenLabs dashboard, add a custom MCP server integration:

| Field | Value |
|---|---|
| Transport | **Streamable HTTP** |
| Server URL | `https://<your-tunnel-hostname>/mcp` |
| Secret token / Authorization header | `Bearer <your gent-talk token>` |
| Approval mode | **Fine-Grained Tool Approval** |

Then set per-tool approval: `list_channels`, `digest_channel`, `find_message` and `read_message`
to **no approval**, and `post_reply` to **require approval**. That is what implements "reading is
automatic while driving, posting asks first".

**Which token you give it decides the ceiling.** With the read token the agent physically cannot
post — `post_reply` is not even listed for it. With the write token it can, subject to the
approval prompt. The conservative first deployment is the read token.

**Say this plainly: the per-tool approval setting is enforced on ElevenLabs' side, and this server
cannot verify it.** Nothing here can tell whether the owner configured approval correctly, or
whether it was later changed. What this server enforces is the scope split and the allowlist:
a read token cannot post, and no token can reach a channel outside the configuration. Do not
mistake the approval prompt for a guarantee we implement.

Verify the endpoint by hand before pointing an agent at it:

```sh
# 401, and the body says nothing useful
curl -s -o /dev/null -w '%{http_code}\n' -X POST https://<hostname>/mcp \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'

# the real thing
curl -s -X POST https://<hostname>/mcp \
  -H "authorization: Bearer $GENT_TALK_READ_TOKEN" \
  -H 'content-type: application/json' \
  -H 'accept: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"curl","version":"0"}}}'

curl -s -X POST https://<hostname>/mcp \
  -H "authorization: Bearer $GENT_TALK_READ_TOKEN" \
  -H 'content-type: application/json' -H 'accept: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
```

The read token's `tools/list` must NOT contain `post_reply`. If it does, something is wrong with
the deployment, not with the agent.

## Putting it on the public internet with a Cloudflare Tunnel

The server speaks plain HTTP and holds a bot token, so it must never be exposed directly. A
Cloudflare Tunnel gives it a public HTTPS hostname with **no inbound port open** on the host: the
`cloudflared` daemon dials out and Cloudflare terminates TLS.

```sh
# once, on the host
cloudflared tunnel login
cloudflared tunnel create gent-talk
cloudflared tunnel route dns gent-talk gent-talk.<your-domain>
```

`~/.cloudflared/config.yml`:

```yaml
tunnel: gent-talk
credentials-file: /home/<you>/.cloudflared/<tunnel-uuid>.json
ingress:
  - hostname: gent-talk.<your-domain>
    service: http://127.0.0.1:8080
  - service: http_status:404
```

```sh
cloudflared tunnel run gent-talk
```

Bind the server to loopback when you do this — `GENT_TALK_BIND=127.0.0.1:8080` — so the only path
in is the tunnel. With Podman, publish to loopback: `-p 127.0.0.1:8080:8080`.

**A tunnel is transport, not authorization.** It gives you TLS and hides the host; it does not
decide who may call. This server's bearer tokens do that. If you want a second fence in front,
Cloudflare Access can require an identity before a request ever reaches `cloudflared` — worth
doing, and still not a substitute for the token check, which is the one this codebase tests.

## Where ElevenLabs attaches

From the related-work review, and recorded in `src/mcp.rs` so it does not have to be rediscovered:

* ElevenLabs Agents connect to **remote MCP servers over SSE or streamable HTTP** — so this server
  is the MCP endpoint and the agent is the client.
* Auth is a **secret token or custom headers**, which is why this server's API is bearer-token
  based: the agent configuration carries the read token.
* There are **three approval modes**, and the useful one is **per-tool approval**, which maps onto
  the rule this project wants: **reading is automatic, posting asks first.**
* **Barge-in and `skip_turn` are native**, so pause/resume needs no button.
* Caveats: MCP is unavailable on Zero Retention Mode accounts, channel text transits ElevenLabs,
  and conversation costs roughly $0.01/minute.

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
```

141 tests: unit tests beside each module, end-to-end tests in `tests/api.rs` that drive the real
router against the in-memory Discord, and `tests/mcp.rs` doing the same for the MCP endpoint. The fake is not a yes-man — it shares the real client's
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

## Layout

```text
src/config.rs         configuration, environment overrides, secret redaction
src/auth.rs           bearer tokens, read vs write scope
src/model.rs          Message/Channel types, snowflake ordering
src/discord/          the DiscordClient trait, the live HTTP client, the in-memory fake
src/summary.rs        extractive digest lines
src/retrieval.rs      semantic random access, behind the Ranker trait
src/untrusted.rs      the data-not-instructions boundary
src/ops.rs            the operations both front doors share: allowlist, fetch, transform
src/mcp/mod.rs        the tool manifest and per-tool approval policy
src/mcp/protocol.rs   JSON-RPC 2.0 and the MCP method set
src/mcp/transport.rs  the Streamable HTTP endpoint at /mcp
src/agent_backend.rs  the slow-path seam
src/http/             router and handlers
web/                  the phone app (plain HTML/CSS/JS, no framework, no build step)
```

## Known gaps

Beyond the security list above:

* `resolve` only searches the window it just fetched (default 50, max 100 messages). Older messages
  are unreachable; there is no pagination and no store.
* Ranking is lexical. It matches words, not meaning, so a paraphrase with no shared words will miss.
  The `Ranker` trait is the replacement point.
* Summarization is extractive truncation, not a model. It shortens; it does not comprehend.
* No caching: every question is a fresh Discord fetch, and Discord's rate limits are not handled.
* No `Retry-After` handling; a 429 from Discord surfaces as HTTP 502.
* The web app has no service worker and no offline mode.
* **The MCP endpoint has never been driven by a real ElevenLabs agent.** It is tested against the
  protocol as written and against `curl`; the registration steps above are from the vendor's
  documentation, not from a completed round trip.
* **The Discord layer has still never run against live Discord.** Adding MCP did not change that:
  both front doors go through the same untested-against-production client.
* The legacy HTTP+SSE MCP transport is not implemented. A client that cannot do Streamable HTTP
  cannot connect.
* No MCP resources, prompts, sampling, or logging capability — tools only.
* No `Mcp-Session-Id`, therefore no server-initiated notifications and no `tools/list_changed`.
  A configuration change is picked up by restarting, and the client re-lists on its next connect.
* This crate is intentionally outside the repository's Rust workspace and is not part of
  `make check` / `make test`; it has its own CI workflow.

## License

MIT, as with the rest of the repository.
