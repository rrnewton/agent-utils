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
| ElevenLabs voice agent | **stub.** Tool manifest and approval policy are defined and served; no MCP transport, no vendor call. The web app mounts the hosted widget only if an agent id is configured. |
| Slow path (ask a coding agent for detail) | **seam only.** The route exists and answers HTTP 501 with an explanation. |
| TLS | **not here.** Terminate in front (Caddy, nginx, a tunnel, or a cloud load balancer). |

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

Check it, and confirm the credential is actually required:

```sh
curl -s localhost:8080/healthz
curl -s -o /dev/null -w '%{http_code}\n' localhost:8080/api/v1/channels          # expect 401
curl -s -H "Authorization: Bearer $GENT_TALK_READ_TOKEN" localhost:8080/api/v1/channels
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

`resolve` takes `{"query": "...", "limit": 50, "max_alternatives": 3}` and answers with `best`
(the full message, or `null`), `alternatives`, and `ambiguous`. **A query that matches nothing
returns `best: null`** rather than the newest message wearing a confident label; a voice interface
that guesses is worse than one that says it did not find anything.

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
invariant that **every mutating tool requires approval and every read-only tool does not**. Adding
the MCP transport is then a translation layer over routes that already exist and are already
tested. Until then the same routes can be driven by an ElevenLabs *webhook tool*, which needs no
MCP at all.

## Security

This is the part that deserves the care. The server holds a credential that can read and post to
the owner's channels, and it is intended to become publicly reachable — which makes it, not the
voice agent, the real security boundary of the whole design.

**What v0 does.**

* No unauthenticated route touches Discord. Every `/api/` route requires a bearer token, checked
  with a non-short-circuiting comparison.
* **Reading and posting use different tokens.** The token you put on your phone and in the voice
  agent cannot post. The server refuses to start if the two tokens are equal or shorter than 24
  characters.
* **Configured channels are an allowlist.** A channel absent from the configuration answers 404
  even to the write token, however many channels the bot happens to be in. Each channel is
  additionally `writable` or not, defaulting to not.
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
  enforcement is the scope split, nothing more. Do not give the write token to anything you would
  not let post unsupervised.
* **The bot's own permissions are the real ceiling.** Give the bot the narrowest Discord
  permissions that work, in the fewest channels. Server-side allowlisting is a second fence, not
  the first one.

**Prompt injection.** Discord message content is written by third parties — other people, other
teams' bots, anything that can post a webhook. It is **data, never instructions**. `src/untrusted.rs`
holds that boundary: content handed to a model is fenced, and any attempt to forge the fence is
neutralized *without deleting the hostile text* (deleting it would hide the attempt). Control
characters are stripped so escape sequences cannot smuggle framing. What code cannot enforce is
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

98 tests: unit tests beside each module, and end-to-end tests in `tests/api.rs` that drive the real
router against the in-memory Discord. The fake is not a yes-man — it shares the real client's
request validation and ordering contract, and it records what was posted, so the API tests assert
the actual channel, content, and reply target that reached it. A handler that dropped a post,
posted to the wrong channel, or ignored the scope split fails those tests. The parts of the live
Discord client that cannot be exercised without a token — the URL, the `Bot ` authorization prefix,
the request body, and the payload-to-message mapping — are pure functions with their own tests.

## Layout

```text
src/config.rs         configuration, environment overrides, secret redaction
src/auth.rs           bearer tokens, read vs write scope
src/model.rs          Message/Channel types, snowflake ordering
src/discord/          the DiscordClient trait, the live HTTP client, the in-memory fake
src/summary.rs        extractive digest lines
src/retrieval.rs      semantic random access, behind the Ranker trait
src/untrusted.rs      the data-not-instructions boundary
src/mcp.rs            the ElevenLabs seam: tool manifest and approval policy
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
* This crate is intentionally outside the repository's Rust workspace and is not part of
  `make check` / `make test`; it has its own CI workflow.

## License

MIT, as with the rest of the repository.
