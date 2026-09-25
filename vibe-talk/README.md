# vibe-talk

A small Rust web server for reading, discussing, and replying to your coding agents' chat messages
from a phone. It supports Discord directly and other services, including Google Chat, through a
compatible HTTP bridge. Messages can be read with the phone's speech engine or ElevenLabs;
voice conversations use ElevenLabs by default or a deployment-managed WebSocket provider.

It is a **bridge, not an agent host**. It holds the chat backend's credential, answers questions about
channels over an authenticated HTTP API, and serves a phone web app. It is deliberately **not**
co-located with the coding agents: it needs no access to any development workspace, and nothing in
it is tied to the machine it currently runs on. It starts in Podman on a laptop and is expected to
move to a small cloud host unchanged.

The design decision behind it, stated plainly: composing hosted products would also work (see
[`RELATED_WORK.md`](RELATED_WORK.md), which recommended exactly that), but a server is
needed either way to hold the chat credential, and this project would rather own that front
door than hand it to a vendor.

## What it does in v0

**Pull by default, with optional adapter push.** Reads and replies use the configured chat HTTP
API. A deployment may separately give an external provider adapter a dedicated ingestion token;
that adapter pushes normalized events into vibe-talk so an open page can update immediately. The
adapter endpoint is authenticated but does not itself make the deployment public or verify a
provider signature, so any external subscription receiver remains the adapter's responsibility.

**Both summary and full text.** Speech cannot be skimmed, so a digest exists — one short line per
message. But the capability that actually matters is **semantic random access**: describe a message
in your own words ("the one about the mac runner") and get *that* message back **in full**. That is
`POST /api/v1/channels/{id}/resolve`, and it is the reason this exists rather than a
text-to-speech bot.

**Messages and threads.** The web app starts in the main channel. When the backend finds threads,
three controls appear above the history:

* **Main** shows channel messages, with a reply-count button below each thread root.
* **Threads** lists root-message previews and reply counts, ordered by most recent activity at the
  bottom. Opening the list starts at that end; scrolling upward loads older entries.
* **All** interleaves channel and thread messages chronologically. Colored **Thread** badges
  distinguish conversations; tapping a badge opens that thread.

A source added through Settings is always a separate entry in the channel picker, even when that
source identifies one upstream conversation rather than a whole room. The **Threads** control is a
different projection: it lists child conversations within the currently selected channel, and is
absent when the configured backend does not provide thread timelines or when the selected channel
is itself one conversation. It is not an alternate picker for added sources.

The selected channel or space is remembered on this device when you reopen the app. If it is no
longer configured, the app selects the first available channel.

Tap a root's reply count or a thread-list entry to open its history. **Back**, or a swipe right,
returns to the view and scroll position you came from. The selected thread has its own history,
draft, and posting destination. Reply counts marked approximate come from the provider's estimate.

**A normal text box at the bottom.** Scroll below the newest message to write to the channel or
selected thread. **Send** posts without requiring a message to reply to. Drafts are kept separately
for each channel and thread. This composer scrolls with history.

**Sending responds immediately.** Normal messages and replies appear locally with a sending
indicator, leaving the composer ready for the next message. Confirmation replaces that indicator
with the delivered message. A failed or interrupted send keeps its text and offers a manual retry;
partially sent messages retain the remaining text. When delivery is unconfirmed, check the history
before retrying because the chat service may have received the message. Reopening the app restores
unfinished sends without sending them again automatically.

## Installing the reader

Serve vibe-talk over HTTPS, open `/voice` in Chrome on Android, and choose **Install app** from the
browser menu. The installed app uses vibe-talk's speech-bubble icon and opens in a standalone
window. If an older shortcut still has a blank or generic icon, remove it and install again.

The manifest asks browsers that support `display_override` (including Chrome Android) for
`standalone`, while retaining `display: browser` as the standards-defined fallback. Safari on iOS
currently ignores `display_override`, and this page deliberately does not opt into Apple's
standalone meta mode: it therefore remains in browser mode, preserving the established microphone
permission fallback instead of risking repeated prompts after app switches.

Installation does not add offline access. There is no service worker, every app asset is served
with `Cache-Control: no-store`, and the server applies the same policy centrally to all `/api/`
and `/mcp` responses, including errors. Transcripts, minted session URLs, and API responses are
never placed in Cache Storage or any other browser-managed HTTP cache. The installed app still
requires the server to be reachable to load at all.

To ask Chrome's own installability engine about the checked-out page, run:

```sh
make -C vibe-talk pwa-installability
```

The check uses Playwright's Chromium by default and prints its version. Set
`VIBE_TALK_CHROMIUM=/path/to/chrome` to use a particular Chrome build, or run
`python3 vibe-talk/tests/pwa_installability.py --url https://your-host/voice` to inspect a deployed
origin. A green result means CDP reports no manifest or installability errors under an Android
viewport; it is not proof that a physical phone displayed the menu item or launched the app.

### Saved messages on this device

Opening the channel view used to mean a blank list until the server answered. `/voice` now keeps a
bounded snapshot of the channel rows it has already shown, in `localStorage` under
`vibe-talk.voice.message-cache`, and draws it immediately on the next visit:

- **Drawn before the network.** The rows appear as soon as the page loads, with a pill over the top
  of the list saying `Saved 14:02 · refreshing…`. It disappears once the refresh lands.
- **One read to catch up.** A cold start makes one newest-page read, for the channel and view on
  screen only, and merges it in without duplicating rows. Rows the server no longer returns are
  dropped. It does not refetch every saved channel or walk back through history. After that, the
  live stream and the regular poll keep the view current.
- **Views are local.** The snapshot keeps one store per channel. Switching among Main, Threads,
  All and a thread already read is redrawn from that store without a request.
- **Honest when stale.** If the refresh cannot reach the server, the rows stay and the pill says
  `Offline · showing messages saved …`. A server error says `Refresh failed · …`.
- **Bounded.** At most 120 messages and 120 thread cards per channel, 12 channels (the least
  recently used goes first) and about 600,000 characters in total. When storage is full the
  snapshot shrinks, and unsent messages in the outbox always win the space. A damaged entry is
  discarded rather than drawn.
- **Tied to the token.** The snapshot records a fingerprint of the token that read it. Signing
  out, saving a different token (on either page, or in another tab), or a `401`, or a `403` on a
  read, removes it. A channel dropped from the server's configuration is pruned.
- **Not a cursor store.** Provider paging cursors expire, so they are never saved. Scrolling above
  the saved rows waits for the first live read.

This is chat text at rest in the browser profile. Anyone who can open that profile can read the
most recent rows until the token is changed or you sign out. It is not an offline mode: without an
app-shell service worker the page itself cannot load with no network, only keep what it has.

`make -C vibe-talk offline-cache-browser` walks that whole sequence in a real Chromium at an
Android phone's size, against a loopback fake API. Pass `SCREENSHOTS=/tmp/shots` to keep one PNG
per step.

## Reading messages with a device voice

To use **Read** in the message view without an ElevenLabs account or conversational agent, add
this to the server configuration and restart it:

```toml
[read_aloud]
backend = "browser"
```

Reload the phone page, open the message view, press **Read**, and tap a message. Tap that message
again to stop it, or tap another to switch. **Stop** leaves reading mode; **Pace** changes the
speed. A combined row is read in order, and messages are marked read only after playback finishes.
This works independently of the conversational voice mode and per-message summarisation.

Chrome on Android exposes the installed Android text-to-speech service through the Web Speech
API. The page selects a voice the browser reports as local, loads voices as they become available,
and starts playback from a tap. If no voice is available, install or download a voice in Android's
text-to-speech settings and try again. Embedded Android WebViews do not support this API; open
the page in Chrome instead. Keep the page visible while listening: background and locked-screen
playback depend on the browser and are not guaranteed.

The app makes no ElevenLabs speech request in browser mode. The phone's configured speech engine
controls its own processing: Chromium's `localService` flag does not prove that every Android
engine stays offline. Choose a downloaded offline voice and check it in airplane mode when that
distinction matters. Browser automation verifies the interaction with a simulated speech engine;
the opt-in physical-device check below verifies the audible output itself.

### Checking the audio an Android device actually emits

`scripts/android-device-speech.py` closes the gap between a JavaScript call to `speak()` and sound
that can be understood. It drives the real served `/voice` page in Android Chrome over ADB, selects
device speech, records Android's rendered playback, converts it to mono PCM, rejects silence and
clipping, and passes the recording to an STT adapter. The run fails if the detected language does
not match Chrome's language or if the recognised words do not match the selected message.

The device must have USB debugging enabled and Chrome open. Python Playwright, ADB and ffmpeg are
the host dependencies. The STT adapter is an executable outside this repository: it receives the
WAV path as its only argument and prints JSON containing `text`, `language`, and optionally numeric
`confidence`. This keeps local models, service credentials, and deployment-specific routing out of
the reusable source tree.

```sh
cd vibe-talk
VIBE_TALK_WRITE_TOKEN=... scripts/android-device-speech.py \
  --url https://vibe-talk.example.invalid/voice \
  --serial ANDROID_SERIAL \
  --transcriber /path/to/stt-adapter
```

The token is accepted only through the environment and is placed into that Chrome origin through
the debugging connection; it is never put on the command line or printed. By default the harness
uses Android `screenrecord` only when its help explicitly advertises internal playback capture. On
other devices, `--capture-adapter PATH` supplies a recorder; it is called as
`PATH DURATION_SECONDS OUTPUT_PATH` and may, for example, record the phone's speaker through a host
microphone. `--channel`, `--message-id`, and `--message-index` choose a known row. Run `--help` for
all defaults and applicability.

Artifacts—including the source text and STT result—go under the gitignored
`debug/android-device-speech/` directory by default. `--signal-only` is useful for diagnosing the
capture route, but it deliberately does not count as the language regression. The evaluator's
silence, clean-signal, wrong-language, unrelated-words, and malformed-STT negative controls run in
the ordinary `make validate` suite through `--self-test` and need no device.

Omitting `[read_aloud]`, or setting `backend = "elevenlabs"`, preserves server-generated ElevenLabs
audio and its existing voice configuration. A deployment whose conversational backend speaks the
public `vibe-talk-v1` WebSocket protocol can set `backend = "conversation"`; the server sends the
message as a text turn, collects the agent's 24 kHz PCM response, and returns WAV audio. When a
server-audio backend is available, the channel bar offers an instant device/agent selector.
`read_aloud.websocket_url` may name a server-only route to that same agent when the browser-facing
conversation URL goes through a different ingress.
`GET /api/v1/client-config` reports the selected backend and playback mode. Rust handlers depend on
`speech::SpeechProvider`; vendor credentials and API details stay in its adapter.

## Setting it up

**If you are setting this up for the first time, use [`QUICKSTART.md`](QUICKSTART.md).** It runs
the six steps — Discord bot, tokens, run it, expose it (optional), ElevenLabs agent, first
conversation — in order, with the exact commands and no gaps to fill in from here. This file is
the reference behind it: what every route does, what the security model is, and where it stops.

Two things are worth knowing before you begin, because they shape everything else:

* **It assumes no infrastructure.** One machine that can run a static binary, or a container
  runtime, is the whole requirement. There is no database to provision, no queue, no reverse
  proxy, no cloud account and no domain. See **From zero** below.
* **What you have to go and get is a short list, and it is the slow part.** Two vendor dashboards
  hand out five values between them and one of those is shown exactly once. See **What you must
  supply** below, which is written as a walkthrough rather than a table for that reason.

## Status: what works, what is stubbed

| Piece | State |
|---|---|
| HTTP server, routing, JSON API | **works** |
| Config file + environment overrides, with validation | **works** |
| Bearer-token auth, read scope vs write scope | **works** |
| **Per-request access log** | **works.** One INFO line per HTTP request, per MCP JSON-RPC message, and per tool call. No credential ever, no channel text above DEBUG. |
| Discord read + post behind a trait | **written, unit-tested; never run against live Discord** |
| In-memory Discord for tests and `--fake-discord` | **works** |
| Digest (`/digest`) | **works**, extractive and deterministic (no model call). This is the whole-channel digest, not the per-message summary below. |
| Per-message summaries, from the ElevenLabs agent | **written, unit- and integration-tested offline, NEVER RUN AGAINST LIVE ELEVENLABS — and it is now the only summariser there is.** Asks the configured conversational agent in text over its own WebSocket, on a pooled conversation recycled every eight summaries. The frames were read out of the vendor's SDK; the offline tests drive the real socket client against this repository's own mock, which was written from the same reading, so the two agree with each other and not yet with ElevenLabs. The extractive truncating summariser that used to be the default has been **deleted**, so there is nothing to fall back to: a deployment without ElevenLabs credentials still starts, and shows every long message as a **failed** summary — a red row — instead of handing you its opening lines and calling that a summary. |
| Semantic random access (`resolve`) | **works**, lexical ranking behind a `Ranker` trait |
| Web app: text tab, digest, find-a-message, local speech | **works** |
| Device speech for the message view's Read control | **prototype.** Uses browser-reported local voices without ElevenLabs credentials. Playback, cancellation, and failures have automated browser coverage; physical-phone audio and background playback remain device checks. |
| Main, Threads, All, selected-thread history, and bottom composer | **works.** Native thread endpoints have loopback HTTP tests; a compatible Google Chat bridge has been exercised with real read-only traffic. Browser interaction and layout were checked at two phone sizes. Posting tests use local fakes. |
| **MCP over Streamable HTTP at `/mcp`** | **works.** Bearer-authenticated, stateless, seven tools, tested end to end. Never yet driven by a real ElevenLabs agent. |
| ElevenLabs voice agent | **reachable, and currently NOT invoking tools.** A real agent has now been driven headlessly (`scripts/run.sh --smoke-agent`): the signed URL mints, the conversation opens, the agent answers — and it calls no tool, saying its tools "appear to be out of date". In the same conversation ElevenLabs reports our MCP server connected with all five tools visible, so the fault is in the agent's own configuration rather than in this server. |
| **Signed conversation URLs at `/api/v1/signed-url`** | **works against a fake, unverified against live ElevenLabs.** Mints a short-lived signed URL for an agent that has "Enable Authentication" turned on, and `/voice` is a dependency-free page that uses one. Tested end to end against an in-memory ElevenLabs that refuses a wrong key and an unknown agent, and against a loopback HTTP server that proves the account key travels in a header. |
| Slow path (ask a coding agent for detail) | **seam only.** The route exists and answers HTTP 501 with an explanation. |
| TLS | **not here.** Terminate in front, with whatever you already use — Caddy, nginx, any tunnel, a cloud load balancer — or do not expose it at all. One worked example is below; it is an example, not a requirement. |
| Agent smoke test (`scripts/smoke-agent.py`, `run.sh --smoke-agent`) | **works, and it goes red on the real failure.** Holds a real conversation and fails unless a tool invocation appears in this server's own access log. Manual and opt-in: it costs vendor minutes, so it is in no suite and no CI. |
| Diagnostics route (`GET /api/v1/diagnostics`) | **written, tested against in-memory fakes only.** Re-runs the startup checks on demand and answers a structured report with a remedy on every failure. The Discord and ElevenLabs halves have never met a live vendor. |
| Deployment check (`scripts/verify-deployment.sh`) | **works.** One command, pass/fail, exits non-zero and names the failing check. Runs in CI against the container on every push, including a negative control that requires it to go red. |

Honest summary: everything except the two vendor-facing halves — live Discord and ElevenLabs — is
implemented and tested. Those two are exactly the parts that cannot be tested without credentials.

## What you must supply

Nothing is committed and nothing is hardcoded, so every value below is one you go and get. They
are listed in the order you need them: items 1–4 are what it takes to start the server at all, and
items 5–7 are the voice half, which the Discord half runs perfectly well without.

Have somewhere to paste things before you begin: two of them — the bot token and the ElevenLabs
API key — are shown exactly once and cannot be read back afterwards.

### 1. A Discord bot token

Use a **second, dedicated bot**, separate from whatever your coding agents post with. This one is
held by a server that may end up reachable from the internet, and the blast radius of the two
should not be the same.

1. <https://discord.com/developers/applications> → **New Application**. Name it anything.
2. **Bot** tab → **Reset Token** → **Copy**. That string is `discord.bot_token` /
   `VIBE_TALK_DISCORD_BOT_TOKEN`. It is shown once; paste it somewhere before navigating away.
3. **Bot** tab → **Privileged Gateway Intents** → turn on **MESSAGE CONTENT INTENT** → **Save
   Changes**. See the warning below; this is the one that costs an hour.
4. **OAuth2** → **URL Generator** → scope **`bot`**, then bot permissions **View Channels** and
   **Read Message History** — and **Send Messages** *only* if you want the bridge to be able to
   post. Open the generated URL, pick your server, authorize.

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

### 2. The channel snowflake ids

In the Discord app: **User Settings → Advanced → Developer Mode** on. Then right-click each
channel you want reachable → **Copy Channel ID**. They are 17–20 digit numbers.

Give each one a **label you can say out loud** — it is what you will use in a sentence to a voice
agent, so `lead team` beats `#eng-agents-prod-2`. Labels and ids go in `[[channels]]` or in
`VIBE_TALK_CHANNELS` as `id:label:rw` / `id:label:ro`.

### 3. Your own Discord user id

`discord.owner_user_id` / `VIBE_TALK_DISCORD_OWNER_USER_ID`. With Developer Mode already on,
right-click **your own name** on any message → **Copy User ID**.

**This one cannot be derived from anything the server holds**, which is why it is a setting at all.
The bot token's first segment *is* the bot's user id, so messages the bridge posts for you are
recognised for free — but a bot account has no relationship to the person reading the channel. Set
this and the messages you typed into Discord yourself are drawn as **yours**; leave it unset and
they arrive as some third party's. It is not a secret: a user id rides on every message that
account has ever sent. See **Why your own messages need a setting** below, and note that Settings
in the web app can assign it per account without a restart.

### 4. Two API tokens of your own

These are neither Discord's nor ElevenLabs'. They are what this server demands of *its* callers —
your phone, your voice agent — and they are the whole of what stands between the internet and your
Discord.

```sh
openssl rand -base64 33   # read token  — goes in the phone and in the voice agent
openssl rand -base64 33   # write token — the capability to post as your bot
```

They **must differ** and each must be **at least 24 characters**, or the server refuses to start
rather than coming up weaker than you meant.

### 5. An ElevenLabs API key — only for the voice half

<https://elevenlabs.io/app/settings/api-keys> → **Create API Key** → copy it. It is shown once.

It goes in `elevenlabs.api_key` / `VIBE_TALK_ELEVENLABS_API_KEY` and it is used for two things:
minting the short-lived signed URLs that open a conversation with an authenticated agent, and
reading a message aloud. It is an **account** secret that can spend money, so it never leaves the
server — it travels to ElevenLabs in a header, never in a URL, and is redacted out of every error.

### 6. An ElevenLabs agent id — only for the voice half

<https://elevenlabs.io/app/agents> → create an agent (a default template is fine; keep their
hosted LLM). Open it, and the id is in the dashboard URL — it starts `agent_`. It goes in
`elevenlabs.agent_id` / `VIBE_TALK_ELEVENLABS_AGENT_ID` and is public: it identifies a widget.

### 7. An ElevenLabs voice id — OPTIONAL, and most deployments should skip it

`elevenlabs.voice_id` is **optional and normally left unset**, and the reason is worth knowing
rather than guessing at. An agent and a voice are different objects in ElevenLabs' model: an agent
is a prompt, a model, tools *and* a voice, while the text-to-speech API takes a voice and knows
nothing about agents. With the ElevenLabs read-aloud backend and `voice_id` unset, read-aloud **borrows the configured agent's own
voice** — and the agent's delivery with it, its speed and stability — so a channel message is read
out by the same voice that talks to you, from the one thing you already configured.

Set it only when those two should deliberately **differ**. With neither a voice id nor an agent id
configured, the ElevenLabs backend refuses and names both, rather than falling back to whichever voice the
account happens to list first.

Copy `vibe-talk.example.toml` to `vibe-talk.toml` (gitignored) and fill it in, or pass everything
through the environment. The environment wins over the file, so a container can be given its
secrets without them ever touching a disk.

## From zero

This section assumes you have nothing: no cloud account, no domain, no reverse proxy, no
orchestrator. Three decisions, in order, and only the first is mandatory.

### 1. Run the server somewhere

The minimum is **one machine that can run one process and reach `discord.com` outbound**. There is
no database to provision — durable state, when you turn it on, is a single SQLite file — and
nothing about the deployment is tied to the host: a laptop, a Raspberry Pi, a spare VM and a small
cloud instance are all the same deployment, and moving between them is moving a config file.

Two ways to run it, and neither is preferred:

* **Cargo, on a machine you already have.** A Rust toolchain, `cargo run`, done. `web/` is compiled
  into the binary, so there is no asset directory to serve and no build step for the front end.
* **The container image.** `podman build` (or `docker build`) from `vibe-talk/Containerfile`. The
  image runs as a non-root user, contains no configuration, and fails immediately rather than
  starting with defaults if it is given no credentials.

Both are spelled out under **Running it** below, including the volume mount you need if you want
state to survive a rebuild. If you just want to look at the web app, `--fake-discord` needs no bot
and no network.

### 2. Expose it — OPTIONAL

**You do not have to.** A deployment that is only ever reached over your LAN, or only over
`localhost` through an SSH session, is a supported and complete deployment. The Discord half, the
web app, the digest, the scrollback and the deployment check all work with no ingress whatsoever.
Skip to step 3 if that is you.

If you do want to reach it from elsewhere, **any** mechanism is fine, because vibe-talk takes no
position on how bytes arrive. The actual requirement is short:

* **a URL a browser can load**, and
* **HTTPS if you want the microphone**, because browsers gate `getUserMedia` on a secure context.
  `localhost` also counts as a secure context, so a local-only setup needs no certificate at all.
* **vibe-talk terminates no TLS itself** and never will. It speaks plain HTTP and expects
  something in front of it if the front is public.

Pick whichever of these you already know, or already run:

| Way in | What it gives you | What it costs |
|---|---|---|
| **Nothing at all** — LAN or `localhost` | The whole application, microphone included over `localhost` | No remote access |
| **SSH reverse tunnel** (`ssh -R`) | A port on a host you already have | You manage TLS on that host, or use it over `localhost` |
| **Reverse proxy with a certificate** — Caddy, nginx, Traefik | HTTPS on a name you control | You need a public host and a domain |
| **A tunnel service** — Cloudflare Tunnel, ngrok, Tailscale Funnel and others | HTTPS with no inbound port open | A vendor account and a dependency on it |
| **A cloud load balancer** | HTTPS in front of a private instance | Cloud account, per-hour cost |
| **A VPN or overlay network** — WireGuard, Tailscale, ZeroTier | Reachable from your devices, exposed to nobody | Every client device must join the network |

**A worked example of one of them** — a Cloudflare Tunnel — is written out under
[Exposing it: one worked example](#exposing-it-one-worked-example) below. It is there because a
complete example is more useful than six partial ones, not because it is recommended over the rest.

Whatever you choose, bind the server to loopback (`VIBE_TALK_BIND=127.0.0.1:8080`, or
`-p 127.0.0.1:8080:8080` under a container runtime) when something else is fronting it, so the
only path in is the one you meant.

**One thing does need a public URL: a hosted voice agent.** ElevenLabs calls the MCP endpoint
machine-to-machine from its own infrastructure, so it must be able to reach your server. If you
are not wiring up a voice agent, that constraint does not apply to you.

### 3. Check it, from the outside

`scripts/verify-deployment.sh --url <wherever it is>` is the same set of checks wherever you point
it, which is the point — run it against `http://127.0.0.1:8080` first and against the public URL
second, and any disagreement is something your ingress did. See **Checking a deployment** below.

## Running it

Locally, without touching Discord at all:

```sh
cd vibe-talk
cargo run -- --config vibe-talk.toml --fake-discord
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
cargo run -- --config vibe-talk.toml
```

Then open `http://<host>:8080/`, go to **Settings**, paste one of **this deployment's own** two
tokens into **vibe-talk API token**, and press Save. Not the bot token and not the ElevenLabs key:
neither is ever entered into the page. `VIBE_TALK_READ_TOKEN` is enough to read, search and
listen; `VIBE_TALK_WRITE_TOKEN` is what replying from the page needs, and what `/voice` needs to
start a conversation at all. The page cannot tell you which one you pasted — `/api/v1/client-config`
answers both identically — so a read token loads the whole interface and then fails at the first
write, which is why both refusals now name the scope. It is stored per browser **and per
hostname**, so a second URL onto the same deployment asks again.

### Podman

Build (context is the `vibe-talk` directory):

```sh
podman build -t vibe-talk:v0 -f vibe-talk/Containerfile vibe-talk
```

Run with everything from the environment — no configuration file in the image, no secret baked in:

```sh
podman run --rm --name vibe-talk -p 8080:8080 \
  -e VIBE_TALK_DISCORD_BOT_TOKEN='your-bot-token' \
  -e VIBE_TALK_READ_TOKEN='your-read-token' \
  -e VIBE_TALK_WRITE_TOKEN='your-write-token' \
  -e VIBE_TALK_CHANNELS='123456789012345678:lead team:rw,987654321098765432:build noise:ro' \
  vibe-talk:v0
```

Or with a mounted configuration file (`:ro,Z` keeps SELinux hosts happy):

```sh
podman run --rm --name vibe-talk -p 8080:8080 \
  -v ./vibe-talk.toml:/etc/vibe-talk/vibe-talk.toml:ro,Z \
  vibe-talk:v0
```

Behind a tunnel or a reverse proxy, publish to loopback only, so the container has no path in
except the thing you put in front of it:

```sh
podman run --rm --name vibe-talk -p 127.0.0.1:8080:8080 \
  -e VIBE_TALK_DISCORD_BOT_TOKEN='your-bot-token' \
  -e VIBE_TALK_READ_TOKEN='your-read-token' \
  -e VIBE_TALK_WRITE_TOKEN='your-write-token' \
  -e VIBE_TALK_CHANNELS='123456789012345678:lead team:rw' \
  vibe-talk:v0
```

The image runs as a non-root user and contains no configuration; a container started with no
credentials fails immediately rather than coming up with defaults.

**None of the commands above keep anything.** The image declares a volume at `/var/lib/vibe-talk`
and points `VIBE_TALK_STORAGE_PATH` into it, but a container run without a mount writes into its
own writable layer, which `podman rm` — and therefore every rebuild and every redeploy — destroys
without a word. Mount a host directory over it:

```sh
mkdir -p -m 700 ~/.local/share/vibe-talk
podman run --name vibe-talk -p 127.0.0.1:8080:8080 \
  -v ~/.local/share/vibe-talk:/var/lib/vibe-talk:Z \
  -e VIBE_TALK_DISCORD_BOT_TOKEN='your-bot-token' \
  -e VIBE_TALK_READ_TOKEN='your-read-token' \
  -e VIBE_TALK_WRITE_TOKEN='your-write-token' \
  -e VIBE_TALK_CHANNELS='123456789012345678:lead team:rw' \
  vibe-talk:v0
```

`scripts/run.sh` does this for you: it creates `$VIBE_TALK_DATA_DIR` (default
`${XDG_DATA_HOME:-$HOME/.local/share}/vibe-talk`) mode `0700`, mounts it, and reports it under
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
| 401 | The **bot token** is wrong, expired, or regenerated | Reset the token in the Developer Portal and update `VIBE_TALK_DISCORD_BOT_TOKEN`. This one is global, so the probe stops after the first channel instead of repeating itself. |
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
cargo run -- --config vibe-talk.toml --skip-startup-probe
# or
VIBE_TALK_SKIP_STARTUP_PROBE=1 cargo run -- --config vibe-talk.toml
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

It takes the tokens from `$VIBE_TALK_READ_TOKEN` / `$VIBE_TALK_WRITE_TOKEN`, or from
`--read-token` / `--write-token`. Because it takes the base URL as an argument, the **same** checks
run against `http://127.0.0.1:8080` right after `podman run` and against
`https://<your-public-hostname>` once whatever fronts it is up — if the second disagrees with the
first, your ingress changed something.

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

The script is not shipped untried: the vibe-talk CI workflow runs it against the built container
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
  -H "authorization: Bearer $VIBE_TALK_READ_TOKEN" \
  -H 'content-type: application/json' -H 'accept: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

## Surviving a reboot

`scripts/run.sh` launches the container with restart policy `on-failure:5`, which is a policy
about the container *crashing*, not about the machine *rebooting*: `podman-restart.service`
restores only containers whose policy is literally `always`, and on this host it is disabled
anyway. So a hand-started container does not come back after a reboot. On 2026-09-23 that is
exactly what happened — the tunnel unit came back, its origin did not, and the public hostname
served **502 for eighteen hours**: a live tunnel in front of a dead origin.

The fix is a systemd **user** unit that starts the container run.sh already deployed. It builds
nothing and creates nothing, so enabling it cannot change the deployed build.

`~/.config/systemd/user/vibe-talk.service`:

```ini
[Unit]
Description=vibe-talk web app origin
Documentation=file:///home/newton/work/agent-utils/vibe-talk/README.md
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStartPre=/usr/bin/podman container exists vibe-talk
ExecStart=/usr/bin/podman start --attach vibe-talk
ExecStop=/usr/bin/podman stop --time=10 vibe-talk
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
```

```sh
systemctl --user daemon-reload
systemctl --user enable --now vibe-talk.service
```

**Linger is the half that is easy to miss.** A user unit does not start at boot unless the user
lingers; without it you have built something that looks fixed and is not. Check, and enable it if
it is off:

```sh
loginctl show-user "$USER" -p Linger      # must print Linger=yes
loginctl enable-linger "$USER"
```

`ExecStartPre` fails loudly when the container has been removed, rather than leaving the unit
"running" in front of nothing. Recreate it with `scripts/run.sh`, then
`systemctl --user restart vibe-talk` so systemd, not your shell, owns the process again — a
redeploy replaces the container out from under the unit.

**Prove it rather than asserting it**, and prove it through the *public* hostname: a local 200
says the process started, which is never what was broken.

```sh
systemctl --user stop  vibe-talk    # then the public URL must go 502
systemctl --user start vibe-talk    # then it must come back 200
```

Rebooting is not the test. The stop/start pair is the control that actually discriminates.

## Asking the server what is wrong

`GET /api/v1/diagnostics`, **read scope**, `Cache-Control: no-store`. It re-runs, on demand, the
checks this server otherwise only runs once at startup, and hands them back **structured** instead
of only printing them to a console you have to still have open from the last restart.

**It always answers HTTP 200.** The report *is* the answer, so a failing check is not an HTTP
error — a 500 here would be indistinguishable from the server being broken in some other way, and
the one thing this route exists to do is tell those apart.

```jsonc
{
  "ok": false,
  "checks": [
    {
      "id": "discord.channel",
      "title": "Discord channel: lead team",  // the label YOU wrote, so you recognise the row
      "subject": "123456789012345678",        // which one, when there is more than one; else null
      "status": "fail",                       // pass | warn | fail
      "summary": "…",                         // the headline, from the shared probe vocabulary
      "detail": "…",       // the evidence: an account name, the vendor's own error body
      "remedy": "…",       // what to do about it; null only when there is nothing to do
      "unconfigured": false // nothing was called, because a setting is absent
    }
  ],
  "passed": 4, "failed": 1, "warned": 1,
  "budget_seconds": 20,
  "took_ms": 812
}
```

`unconfigured` is kept apart from an ordinary failure on purpose: "the vendor said no" and "you
never gave us a key" are the same red row in a list and are fixed completely differently.

The checks:

| `id` | What it establishes |
|---|---|
| `discord.token` | Discord accepted the bot token, and who it says the bot is |
| `discord.channel` | **one per configured channel** — readable, or why not |
| `elevenlabs.api_key` | the key was accepted, and which account/workspace it reached |
| `elevenlabs.agent` | the configured agent exists and reports a voice |
| `elevenlabs.voice` | a voice resolves — configured explicitly, or borrowed from the agent |
| `storage` | the store can be written to, or the named reason it cannot |

`discord.channel` carries **the same five distinguishable causes the startup probe already tells
apart**, because they have five different fixes: no OAuth2 invite; a per-channel permission
missing; a mistyped snowflake; a bad token; and the Message Content Intent toggle, which produces
no error at all and reads as an empty channel. See **The startup channel probe** above for the
full taxonomy — this route serves that same classification over HTTP rather than only into stdout.

Four properties are the point of it:

* **Every failing check carries a `remedy`** saying what to do next. That is the whole reason this
  exists: it replaces "unavailable" — a word that tells a reader nothing they can act on — with a
  sentence naming the setting, the toggle, or the permission.
* **It never contains a credential.** Every string in the report goes through the same redaction
  the vendor error paths use, against every secret the deployment holds. A vendor is free to quote
  a key back inside its own error text, and this route reports vendor error text.
* **It makes live vendor calls, so it is bounded.** Each check has its own time budget and the
  whole report has an overall deadline (`budget_seconds`). A vendor that does not answer is a
  **failed check with a reason**, never a hung request.
* **Read scope, not write.** It reads configuration state and pokes vendors; it changes nothing.

**Honesty note.** The ElevenLabs and Discord halves of this have been exercised **only against
in-memory fakes, never against live Discord or live ElevenLabs.** The classification is the same
code path the startup probe uses, which is itself in the same position — see **Known gaps**.

## Configuration

| Setting | File | Environment | Notes |
|---|---|---|---|
| Bind address | `server.bind` | `VIBE_TALK_BIND` | `0.0.0.0:8080` in a container |
| Public URL | `server.public_base_url` | `VIBE_TALK_PUBLIC_BASE_URL` | informational |
| Time zone | `server.timezone` | `VIBE_TALK_TIMEZONE` | IANA name, default `UTC`; an unknown one refuses to start |
| Count ceiling | `discord.max_count_scan` | — | how many messages a count may walk, default `500` |
| Provider request timeout | `discord.request_timeout_seconds` | — | seconds for one ordinary provider HTTP request, default `20`, range `1`–`120`; compatible bridges may need longer |
| Live poll interval | `discord.live_poll_seconds` | `VIBE_TALK_LIVE_POLL_SECONDS` | seconds between inbound reads per channel; **`0` (default) is OFF**, and under `5` is refused |
| Live push token | `ingest.token` | `VIBE_TALK_INGEST_TOKEN` | **secret**, optional, ≥ 24 chars and distinct from both API tokens; enables adapter push and cannot be combined with live polling |
| Channel registration | `discord.channel_registration` | — | **off by default**; enable only when `discord.api_base` is a compatible bridge implementing `POST /channels` and `DELETE /channels/{id}` |
| Upstream read marks | `discord.upstream_read_marks` | — | **off by default**; enable only when `discord.api_base` is a compatible bridge implementing `POST /channels/{id}/read` |
| Chat service name | `discord.provider_name` | — | `Discord`; set to the source service's name (for example, `Google Chat`) when using a compatible HTTP bridge |
| Thread protocol | `discord.thread_api` | — | `native` (default) for Discord, `bridge` for the normalized endpoints below, or `off` to disable child-thread timelines; an added source that identifies one upstream conversation remains an ordinary channel-picker entry in every mode |
| Discord bot token | `discord.bot_token` | `VIBE_TALK_DISCORD_BOT_TOKEN` | **secret** |
| Read token | `auth.read_token` | `VIBE_TALK_READ_TOKEN` | **secret**, ≥ 24 chars |
| Write token | `auth.write_token` | `VIBE_TALK_WRITE_TOKEN` | **secret**, ≥ 24 chars, must differ |
| Channels | `[[channels]]` | `VIBE_TALK_CHANNELS` | `id:label:rw` / `id:label:ro`, comma separated |
| ElevenLabs agent id | `elevenlabs.agent_id` | `VIBE_TALK_ELEVENLABS_AGENT_ID` | public |
| Read-aloud backend | `read_aloud.backend` | `VIBE_TALK_READ_ALOUD_BACKEND` | `elevenlabs` (default), `browser` for the device speech engine, or `conversation` to use the configured `vibe-talk-v1` agent |
| Conversation backend | `conversation.backend` | — | `elevenlabs` (default) or `websocket` for a deployment-managed `vibe-talk-v1` endpoint |
| Conversation WebSocket | `conversation.websocket_url` | — | required for the `websocket` backend; keep private endpoints in deployment configuration |
| Conversation label | `conversation.label` | — | provider name shown in the UI and its connection errors |
| Prepare bodies for speech | `speakable.enabled` | `VIBE_TALK_SPEAKABLE` | **on by default**; off relays exactly what was typed. Does not affect the time-zone conversion. A value that is neither `true` nor `false` refuses to start |
| ElevenLabs API key | `elevenlabs.api_key` | `VIBE_TALK_ELEVENLABS_API_KEY` | **secret**, needed to mint signed URLs and for the ElevenLabs read-aloud backend |
| Your own Discord user id | `discord.owner_user_id` | `VIBE_TALK_DISCORD_OWNER_USER_ID` | public; **cannot be derived** — see below. Without it, messages you type into Discord yourself are drawn as a third party |
| ElevenLabs voice id | `elevenlabs.voice_id` | `VIBE_TALK_ELEVENLABS_VOICE_ID` | public; **optional** — read-aloud borrows the configured agent's own voice when this is unset |
| ElevenLabs API base | `elevenlabs.api_base` | `VIBE_TALK_ELEVENLABS_API_BASE` | `https://api.elevenlabs.io/v1` |
| Storage path | `storage.path` | `VIBE_TALK_STORAGE_PATH` | absolute; unset means **no durable state** |
| Conversations kept | `storage.max_conversations` | — | default `50`, oldest dropped first |
| Turns per conversation | `storage.max_turns_per_conversation` | — | default `1000`, then `413` |
| Cached summaries kept | `storage.max_summaries` | — | default `2000`, least recently written dropped first |
| "Dealt with" marks kept | `storage.max_dismissals` | — | default `10000`, oldest dropped first; **not** bounded by age, see below |
| Retention age | `storage.retain_days` | — | default `30`; `0` means no age limit, and it bounds cached summaries too |
| Summary threshold | `summaries.threshold_chars` | — | below this nothing is summarised, default `400` |
| Summary width | `summaries.target_chars` | — | default `160` |
| Summary context | `summaries.context_messages` | — | preceding messages shown as context, default `3` |
| Summary model | `summaries.model` | `VIBE_TALK_SUMMARY_MODEL` | nothing reads it; it only moves the cache key |
| Summariser | — | — | **not a setting.** Summaries always come from the ElevenLabs agent in `[elevenlabs]`. `summaries.backend` / `VIBE_TALK_SUMMARY_BACKEND` were removed with the extractive summariser: `extractive` is now refused at startup by name, `elevenlabs_agent` starts with a loud warning that it selects nothing, and anything else is refused as unknown. Delete the line |
| Agent conversation budget | `summaries.max_per_socket` | — | summaries one conversation serves before it is recycled, default `8`, minimum 1 |
| Agent conversation idle close | `summaries.socket_idle_seconds` | — | default `30` |
| Agent turn deadline | `summaries.reply_timeout_seconds` | — | one turn, and one opening handshake, default `30`, minimum 1 |
| Resuming | `replay.enabled` | `VIBE_TALK_REPLAY_ENABLED` | **off by default**; on, every new call re-sends earlier conversation content to the vendor |
| Replay budget | `replay.max_chars` / `replay.max_turns` | `VIBE_TALK_REPLAY_MAX_CHARS` / `_TURNS` | default `6000` / `40`; oldest dropped first; `0` is refused, not "unlimited" |
| Replay transport | `replay.transport` | `VIBE_TALK_REPLAY_TRANSPORT` | `contextual_update` (default) or `client_data` |
| Config file path | — | `VIBE_TALK_CONFIG` | or `--config` |

Environment wins over file. An empty environment variable is treated as unset, so a runtime that
renders unset variables as `""` cannot blank out a configured value. An unknown key in the file is
an error, not a shrug — a typo'd section name should not silently disable a setting.

## Durable state

Almost nothing in this server is remembered. A channel is never cached as a channel: every
question is a fresh Discord fetch, and it stays that way. There is exactly one store,
`src/store/`, and it holds these:

* the `/voice` **conversation transcript** — what you said and what the agent said back;
* **read marks** — how far you have been shown each channel;
* **"dealt with" marks** — which individual messages you have finished with, which is the
  overlay `#50 todo-view` is built on. Two snowflakes and an instant per row, and **no message
  text at all**; and
* **cached summaries** — one short line per long message, filed under the policy that produced
  it. This is the one entry NOT authored by this server: a summary is a model's paraphrase of
  somebody else's message, written from that message and from the messages around it, so it is a
  second at-rest copy of third-party text and is treated as one everywhere below; and
* **channel aliases** — what THIS app calls a channel, one row per configured channel at most.
  Ours and local in the same sense the read marks are: never sent to Discord, renaming nothing
  there. See "What to call a channel". `#39 channel-alias`.

The first three are your own record. The last is a derived cache, bounded by the same
retention as the rest, erased by the same purge, and never filled by a read-scope token.

**Every table is bounded on write, and one of them is bounded differently on purpose.** The
"dealt with" marks have a count ceiling and **no age limit**, unlike everything else here. An age
limit would put a message you cleared last month back into your to-do list because time
passed — a lie you cannot diagnose, since nothing on the row would say why it came back — whereas
an expired summary is merely regenerated. The count bound is enough on its own here for a reason
it is not enough for a summary: this is the one table that holds nobody's words.

### Thread backend contract

Thread discovery, membership checks, message lookup, and posting go through `ChatClient`. The
browser uses `threading_supported` and `chat_provider_name` from `/api/v1/client-config`; it does
not infer capabilities or branding from a service name. Existing `[discord]` configuration keys
remain compatible with earlier deployments.

With `thread_api = "bridge"`, a compatible HTTP bridge implements these paths relative to
`discord.api_base`:

* `GET /channels/{id}/timeline?view=main|threads|flat|thread&thread_id=…&before=…&limit=…`
* `GET /channels/{id}/threads/{thread_id}/messages/{message_id}`
* `POST /channels/{id}/threads/{thread_id}/messages`

Thread identifiers and pagination cursors are opaque. Each path segment is percent encoded;
neither callers nor bridges should interpret a thread as a quoted-message reference. Thread reads
and writes must verify membership in the registered parent channel, including any narrower scope
that registration imposes. Posts use the existing message body and nonce contract. Lookup and
post responses use the existing message wire format.

Timeline responses follow `src/threads.rs`: messages and thread summaries are oldest first,
`has_more` pairs with `next_before`, and `has_threads` controls navigation. Message wire objects
carry an optional `thread` object with `id`, `root_message_id`, `is_root`, `reply_count`, and
`reply_count_exact`. Summary roots use the same message wire format. Discovery must finish before
claiming global chronology or exact counts; a cap or provider failure must report an error instead
of returning a plausible but incomplete flattened page. A notice can explain registration scope.
A registration narrowed to one conversation names that conversation as `thread` in main, flat,
and threads responses. Such a channel has no child threads, so vibe-talk reports no thread
navigation for it and never lists the conversation as a child of itself.

The native backend includes accessible active and archived threads. It bounds discovery at 256
threads and 100 pages per archived collection, retains pagination inventories for five minutes,
and bounds a timeline request at 90 seconds. Exceeding those limits returns an explicit error.
Native reply counts are marked approximate because older provider counters may be capped. An
expired cursor reloads the newest history with a fresh continuation. Forum and media channel Main
views show thread roots; open a thread to post in these thread-only containers.

### Local read state is ours; upstream read state is an optional provider capability

Discord does not share read state with bots. There is no ack route a bot may call, no read-state
field on the channel object a bot can see, and no `read_state` in the gateway `READY` payload for
a bot user (this is what issue #61 `unread-status` established). So the read marks here are
vibe-talk's own record, and the rule is stated once rather than left to be inferred:

* **No sync-in.** Marking a channel read in the Discord app changes nothing here.
* **No sync-back.** Marking a channel read here acks nothing and posts nothing; the Discord app's
  unread badge does not move.

Anything that shows a read mark to a person has to say that, because "read" already means
something else to a Discord user.

A compatible provider bridge may also implement `POST /channels` with
`{"source":"...","label":"..."}`. Its JSON response has required path-safe string `id` and
boolean `created` fields plus an optional boolean `writable` field, for example
`{"id":"path-safe-stable-id","created":true,"writable":true}`. `created` is `false` when the
bridge reused an existing registration. `writable` defaults to `false` when omitted for compatibility
with older bridges; a present non-boolean value rejects the response and, when `created` is true,
triggers a compensating delete. It is the bridge's policy: vibe-talk persists it and never accepts a
managed-mode writable choice from the UI. Managed message posts carry a path-safe `nonce` generated
once per logical post and kept unchanged across internal HTTP retries; bridges must pass that value
through as their provider request id. Direct Discord posts do not carry this bridge-only field. The bridge must
implement idempotent `DELETE /channels/{id}`: deleting an already-absent registration still succeeds
with `204 No Content`. Opt in with `discord.channel_registration = true`. Settings then accepts a
channel link or provider reference, lets the bridge resolve it to the stable id used by vibe-talk,
checks that id is readable, and hides the writable checkbox. Only rows originally registered through
this managed flow are unregistered upstream on removal; direct and pre-migration rows remain local-only.
Direct Discord deployments keep the existing id, label, default-off explicit writable switch, and must
leave this setting off.

A registration-capable bridge may also list the channels its account can see, which lets
Settings offer **Browse channels** (`#19 channel-browser`): a searchable list with one-tap Add and
the channels already tracked marked as such. The bridge implements `GET /channel-directory` with
optional `query` (trimmed, at most 100 characters; blank lists everything), `limit` (1 to 50, default
25) and `cursor` (the opaque `next_cursor` of the previous page of the same query). It answers
`{"entries":[{"source":"...","name":"...","registered_channel_id":"id-or-null"}],"next_cursor":"...-or-null","truncated":false}`,
in its own order and never more entries than `limit`. `source` is handed back unchanged as the
`source` of `POST /channels`; `registered_channel_id` names the bridge's existing registration, if
any; `truncated` says the bridge stopped listing before the end of what its account can see. A
malformed answer is refused rather than repaired. `401`/`403` reach the page as a refusal, and
`404`/`405`/`501` as "this bridge has no directory". vibe-talk probes nothing at startup: it offers
the list whenever `discord.channel_registration` is on, and a bridge without the route simply
answers the first read with one of those statuses. Every string the bridge returns is drawn as text.

A compatible provider bridge may separately implement `POST /channels/{id}/read` and opt in with
`discord.upstream_read_marks = true`. The web app then offers **Mark read through here** on each
message. That action advances the provider's monotone cursor through the selected message; it does
not write vibe-talk's local read mark and does not archive the message. Thread-level behavior is
defined by the provider. The bridge may return JSON or an empty successful response such as `204
No Content`. Direct Discord deployments must leave the setting off.

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
2. delete the file (`rm ~/.local/share/vibe-talk/vibe-talk.sqlite3`) — with the server stopped; or
3. delete the mounted directory, which is also what `scripts/run.sh --status` prints.

Unsetting `storage.path` is **not** one of them. It turns the feature off for the next run and
leaves every byte already written exactly where it is — which is the opposite of a purge, and is
worth saying because it looks like one.

### Security posture — this is new, and it is a change

This is the first thing vibe-talk retains at rest, and what it retains is your own speech
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

`src/summarize/` is a second trait over the same store. **One** implementation ships — the
conversational agent named in `[elevenlabs]`, asked in text — and there is no setting that chooses
it, because there is nothing to choose between. The server says what is running at startup, and
every summary answer carries the summariser's name, so a page can never imply a model summary it
did not get.

There used to be a second one, `extractive`, and it was the default: truncation, no model, no
network, no cost. It has been **deleted**. It never comprehended anything — what it returned was
the opening of the message it claimed to summarise, which is exactly what a collapsed row already
shows you — so it was an intermediate step, not a product.

`summaries.backend` and `VIBE_TALK_SUMMARY_BACKEND` no longer select anything, but both are still
READ, so that a configuration naming the removed summariser is answered rather than ignored.
`extractive` is refused at startup, by name, saying it was removed and what runs instead.
`elevenlabs_agent` — which one release ago was the documented way to ask for real summaries, and is
therefore sitting in live configuration files — still boots, with a loud warning that the key is
dead and should be deleted; refusing it would stop those servers for no behavioural gain, since it
names exactly what they now get anyway. Anything else is refused as unknown.

The consequence is worth stating plainly rather than leaving to be found: **the ElevenLabs
credentials are now load-bearing for summaries.** Without them there is no summariser at all. The
server still starts and says so; `/summary` then answers `503 summarizer_not_configured`, and
`/voice` states the reason once above the list and turns the rows it asked about red. It does not
fall back to truncation, because a silent fallback is indistinguishable from the agent answering
badly. Regeneration also now costs vendor calls where it previously cost nothing.

What is actually load-bearing here is the **cache key**, because a cached derived value has one
failure mode and it is silent. What decides what a summary says is the prompt, whatever else the
backend contributes, every number in `[summaries]`, and the message text. All but the last are
folded into `summarize::policy_version` — one function, one string, part of every key — so
changing any of them makes every summary produced under the old policy unreachable at once, and a
startup sweep deletes the entries. The message text is a separate `content_hash`, so an upstream
**edit** invalidates one entry rather than the whole cache.

The backend's own contribution comes from the trait rather than from a constant beside the
constructor: `Summarizer::backend` gives the slug and `Summarizer::policy_input` gives everything
else. That matters for the agent backend, whose answers depend on *which agent* — a different
`elevenlabs.agent_id` is a different model, a different system prompt and a different voice of
writing, and it lives outside `[summaries]` entirely. It goes into `policy_input`, so repointing
the deployment cannot serve the old agent's summaries.

The version is deliberately legible (`v1-elevenlabs-agent-w3-c160-8f1b…`), because a stale entry is
diagnosed by someone with a shell. It is FNV-1a and is **not** a security hash: it detects a
configuration change, and nothing more.

Deleting the extractive summariser moved that string three ways at once — the slug, the policy
input, and the removal of the old `backend` term from the hash — so **the first start after this
change empties the summaries table**. That is the sweep working, not a bug: re-serving an
extractive prefix under an agent key is precisely the silent stale summary the version exists to
prevent. The number swept is logged at INFO at startup and nothing else announces it.

A summariser is a model being fed channel text, so the request goes through `src/untrusted.rs`
exactly as the MCP path does — the instruction outside the fence, the message inside it. That is
structural rather than a convention: `SummaryRequest` holds the built prompt in a private field
and the only constructor builds it through `untrusted::fenced`, so a backend cannot be handed
channel text that was never framed, and a test reads back what the summariser was actually given
rather than rebuilding it. The fence is not decoration here: the summariser really is a model
reading other people's text, so the instruction it is given and the surrounding messages it is
given are two different kinds of input and are framed as two different kinds of input. Being short
is not an exemption.

A cached summary is a second at-rest copy of other people's text under the same file, with the
same `0600`. It is bounded by the same retention, it goes with the rest on a purge, and it is
never written by a read-scope caller: `/summary` answers a read token and serves it from the
cache, but only a write-scope caller's answer is filed. The read token is the one pasted into the
voice agent, and a durable write reachable from it is what the two-token split exists to prevent.

### Summaries from the ElevenLabs agent

> **Nothing in this section has been run against live ElevenLabs**, and this section is now the
> whole of message summarisation — there is no second summariser and no setting that selects one.
> The protocol was read out of the vendor's SDK. Every offline test drives the real socket client
> against this repository's own mock, which was written from the same reading — so the two agree
> with each other, not with the vendor. The first real conversation is the experiment, and it is
> instrumented to be one.

There is no REST endpoint for "send this text to my agent, get text back". `simulate_conversation`
looks like the missing one and is not: it is deprecated, and what it simulates is the *user*. The
only path is the conversation WebSocket the browser uses, so `src/elevenlabs/socket.rs` opens one,
sends `conversation_initiation_client_data` with `conversation_config_override.text_only = true`
— which suppresses speech synthesis, the expensive half — asks a `user_message`, and reads until
`agent_response`. It answers the vendor's application-level `ping` events while it waits, because
a conversation that stops echoing them is closed for being idle.

**The pool is the design.** A ConvAI socket is a *conversation*: everything sent down one stays in
the agent's context, including the agent's own previous answers, and there is no event that resets
it. Summarising twenty messages down one socket makes each summary an input to the next — context
grows without bound, cost grows with it, and later summaries are written in front of earlier
people's messages. A socket per summary avoids all of that and pays a mint, a TLS handshake, a
WebSocket upgrade and an initiation round trip every single time, on a vendor that bills
connection time as well as tokens.

So there is **one pooled conversation, recycled every `summaries.max_per_socket` summaries
(default 8) and closed after `summaries.socket_idle_seconds` of quiet (default 30)**. Counting in
prompt-equivalents, the k-th summary on a shared socket carries about *k* of them, so a socket
serving *N* costs about *N(N+1)/2* where *N* separate sockets cost *N*: at N=8 that is 36 for 8,
with the handshake paid once instead of eight times; at N=32 it would be 528 for 32 and the
quadratic term has overtaken the handshake it was meant to amortise. Both numbers are folded into
the cache key, because both change what a summary says. A pooled conversation that fails on its
first write is retried **once** on a fresh socket — a vendor is free to close an idle conversation
from its end, and failing a summary for that would make the pool a source of errors the naive
design does not have. A *fresh* socket that fails is a real failure and is not retried.

**Plain text.** The summary is rendered with `textContent` and read aloud, so a markdown bullet is
an asterisk on the screen and a word in the ear. The agent is told so, in `PLAIN_TEXT_RULE`, which
is part of the cache key; and because an instruction is a request a model may decline, what comes
back is flattened — leading heading and list markers, paired `**`/`__`, and everything
`summary::condense` already removes. Deliberately conservative: a single `*` is left alone,
because mangling "the `*` operator" is worse than the bullet that was already stripped.

**It measures itself.** How a round trip to a hosted conversational agent compares with a
full-size model is an open question nobody here has a number for, so the first real run answers it
rather than needing a separate experiment. Every summary answer carries `generated_in_ms`
(`null` for a cache hit or a message below the threshold — a zero would claim the vendor answered
instantly), `ops` logs one INFO line per request with the backend and the elapsed time, and the
agent backend logs its own breakdown: `connect_ms` (what the pool exists to avoid paying),
`answer_ms`, `reused_socket` and `socket_turn`.

The four failures stay apart, as everywhere else on the ElevenLabs side: not configured (names the
setting, dials nothing), refused (the vendor said no — a rejected key at the mint, or a close
frame on the socket), unreachable, and a shape this server cannot read. `summarizer_refused` is
its own code precisely because "the key is wrong" and "the network is down" are fixed differently.
Every error built from a vendor response goes through `redact`, against both the account key **and
the minted URL**, which carries a single-use token and is quoted verbatim by a connect failure.

### Migrations

`src/store/sqlite.rs` holds an append-only `MIGRATIONS` list and records progress in SQLite's
`user_version`. Opening an older file applies the missing steps. Opening a file written by a
*newer* vibe-talk is **refused**, naming the reason — a downgrade that wrote through the old
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
INFO vibe_talk::access: request method="POST" path="/mcp" credential="write" status=200 millis=12
INFO vibe_talk::access: mcp rpc_method="tools/call" credential="write" is_notification=false
INFO vibe_talk::access: tool tool="post_reply" channel="123…" credential="write" outcome="ok" reason="-" text_len=48
```

Filter for just these: `RUST_LOG=vibe_talk::access=info`. Three lines answer the question that
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

All `/api/` routes require `Authorization: Bearer <token>`. The live-event ingestion route uses
its own adapter-only token; every other route uses the read/write tokens described above.
`/healthz` and the static web app require neither, and neither reveals configuration.

| Method | Path | Scope | Purpose |
|---|---|---|---|
| GET | `/healthz` | none | liveness |
| GET | `/api/v1/channels` | read | configured channels |
| POST | `/api/v1/channels` | **write** | direct mode: `{id,label,writable}`; managed mode: `{source,label}` — validate and add a tracked channel |
| GET | `/api/v1/channel-directory?q=&limit=&cursor=` | **write** | managed mode: one page of the channels the bridge can see, each marked `tracked` when already a channel here |
| DELETE | `/api/v1/channels/{id}` | **write** | remove a channel added in the app; managed mode also unregisters it upstream |
| GET | `/api/v1/client-config` | read | what the web app needs at startup |
| POST | `/api/v1/live/events` | ingest | accept one normalized create/update/delete event from an external provider adapter |
| GET | `/api/v1/diagnostics` | read | re-run the startup checks now, structured, with a remedy on every failure — see above |
| GET | `/api/v1/agent-tools` | read | the voice agent's tool manifest and approval policy |
| GET | `/api/v1/voice-session` | **write** | open a browser-ready session through the configured conversational voice provider |
| GET | `/api/v1/signed-url` | **write** | compatibility endpoint that mints an ElevenLabs signed URL |
| GET | `/api/v1/channels/{id}/messages?limit=` | read | full scrollback, oldest first |
| GET | `/api/v1/channels/{id}/messages/{message_id}` | read | one message in full |
| GET | `/api/v1/channels/{id}/digest?limit=&width=` | read | one speakable line per message |
| GET | `/api/v1/channels/{id}/messages/{message_id}/summary` | read | one message summarised, from cache when it can be |
| GET | `/api/v1/channels/{id}/page?limit=&before=&since=&until=` | read | **one step of a walk**, saying that it is one |
| GET | `/api/v1/channels/{id}/timeline?view=&thread_id=&limit=&before=` | read | main channel, thread list, flattened history, or one thread; opaque backward cursor |
| GET | `/api/v1/channels/{id}/count?since=&cap=` | read | a bounded, honest count |
| POST | `/api/v1/channels/{id}/resolve` | read | **semantic random access** |
| POST | `/api/v1/channels/{id}/reply` | **write** | `{text, thread_id?, reply_to?}` — post to the channel or selected thread; quoting a message is optional |
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
| POST | `/api/v1/channels/{id}/upstream-read` | **write** | `{message_id:"…"}` — ask an explicitly capable provider to move its monotone read cursor through that message; thread behavior is provider-defined |
| PUT | `/api/v1/channels/{id}/alias` | **write** | `{alias:"…"}` — what THIS app calls the channel; see "What to call a channel" |
| DELETE | `/api/v1/channels/{id}/alias` | **write** | drop it, putting the configured label back |
| DELETE | `/api/v1/storage` | **write** | erase EVERYTHING durable: transcripts, read marks, "dealt with" marks, cached summaries, channel aliases |
| POST | `/mcp` | read, or **write** per tool | MCP over Streamable HTTP — see below |
| GET/DELETE | `/mcp` | none | `405`; this endpoint is stateless and has nothing to push |

**The transcript routes require the write scope even to READ.** A stored transcript is your
own speech plus whatever channel text the assistant read out to you, which is a different
and more sensitive thing than a digest of a channel you already allowlisted — and `/voice`, the
only thing that uses them, already holds the write token. The read token gets `403`, and a test
asserts it. **None of them is an MCP tool**, and a test holds that line too: text a tool can
return is text that enters a model's context.

**No read-scope credential writes anything durable.** That is the wider rule the line above is one
case of: every durable write — a turn, a read mark, a cached summary — needs the write scope.
`/summary` is the interesting one, because it is readable at read scope and yet has something to
file: a read token is served from the cache when there is a hit and its answer is never written
back. `/inbox` is the single durable READ a read token may make, because how far you have
read is the thing the voice agent has to be able to say out loud.

**`/api/v1/inbox` carries `read_state_notice` on every answer**, including the local mutating ones,
saying that this read state is vibe-talk's own and is synchronised with Discord in neither
direction. `/upstream-read` is a separate provider write, advertised to the web app through
`upstream_read_mark_supported` in `/client-config`. See **Durable state** above.

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
`spoken_time` is the instant already converted into `server.timezone`, with no seconds and no zone
label — `09:51` — and it is what a voice agent reads out, verbatim, with no conversion of its own.
`timestamp` is the exact ISO-8601 instant Discord reported, unrounded, and it is what anything
computing with a time must use. The conversion happens once, in `src/ops.rs`, so the phone and the
voice agent cannot disagree about when something was said. This exists because handing an
assistant `13:51:25+00:00` and letting it do the arithmetic produced *"thirteen fifty-one Eastern
Time"* — the right clock with the wrong label, when nine fifty-one was the answer.

The seconds and the label were there at first and were both removed on report: they are the two
parts a listener cannot use. Nobody places a chat message to the second, and the label names the
zone they are already standing in — "nine fifty-one and twenty-five seconds Eastern Daylight Time"
spends its length saying so. What replaces the label as a defence is that a bare `09:51` offers no
zone to reinterpret, and the tool descriptions say in as many words that the field is already
local. `src/clock.rs` records what that trades away.

**And a third field, `spoken_content`, carries the BODY rewritten for speech.** Same rule as the
time: `content` is what was typed and what the screen shows, `spoken_content` is what a voice says,
and the rewrite happens once, in `src/ops.rs`, so nothing downstream computes one. `src/speakable.rs`
does three things to it, in an order that matters. Markdown comes off, because read aloud
`**deploy**` is *"asterisk asterisk deploy asterisk asterisk"*. Timestamps become *"three hours
ago"*, because an ISO string is read out character by character. And long numeric ids and hash
codes get a **letter** — *"large number A"* — where the same value is the same letter every time it
appears, so a listener can hear "the same one" without hearing nineteen digits. Times are resolved
before ids because a Unix epoch and a snowflake are both long runs of digits and only one of them
is a time.

The letter table lives on the server for as long as the server does, shared by every reader, and
is bounded at 4096 names per kind — past that a value is spoken as *"a large number"* with no
letter, which is audibly different from a named one rather than quietly ambiguous. Sharing one
table across readers is safe **on this deployment** and not in general: there is one channel
allowlist and everyone reads through it, so a named value is always one already on the reader's
screen. A deployment where two readers saw different channels would need two tables.

`spoken_content` is **empty when the rewrite changed nothing**, which is most chat, so an ordinary
sentence is not sent twice down a phone's connection. Empty therefore means both "nothing prepared
this" and "preparing it was the identity" — deliberately, because `Message::spoken_body()` answers
both with the raw body and no reader needs to tell them apart. That is also what makes
`speakable.enabled = false` a one-line switch: off, the field stays empty and every reader is
already correct.

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

A stored turn is your own speech AND whatever channel text the agent read aloud to you,
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

There are two mutually exclusive ways to feed the same channel-keyed `LiveHub`:

* `discord.live_poll_seconds` keeps the built-in Discord polling source.
* `ingest.token` enables an external provider adapter to push normalized events to
  `POST /api/v1/live/events`.

The browser learns `live_delivery: "off" | "poll" | "push"` from `/api/v1/client-config`, so a
push-only deployment with a zero poll interval still attaches its SSE stream. In push mode the
screen distinguishes "configured and connected to this server" from adapter health, which the
server cannot observe. Configuration refuses to enable both producers at once: duplicate delivery
must not depend on two unrelated cursors happening to agree.

### Provider-adapter push

The adapter route has its own bearer token, distinct from both browser tokens. It accepts only
allowlisted channel ids, limits each JSON body to 64 KiB, and remembers the newest 1,024 event ids
per channel in memory. A retry is idempotent only while its id remains in that window in the same
server process: a restart or 1,024 later events permits it again, so consumers must still tolerate
an occasional duplicate. An accepted event answers `202`; a known retry answers `200` with
`{"accepted":false,"duplicate":true}`. A producer retries only a transport failure, `429`, or `5xx`,
never a permanent `4xx` refusal.

Delivery order is request-acceptance order. The adapter must keep at most one request per channel in
flight, wait for its response, and settle an uncertain result before sending the next event for that
channel. Event ids are opaque de-duplication keys, not sequence numbers, so concurrent requests
cannot be put back into provider order by the server.

Every event carries an opaque, stable `event_id` and a mandatory `historical` boolean. The latter
is a safety boundary: subscription catch-up is rendered but leaves the SSE payload's `replayed`
flag true, so old channel text is never announced to a live voice conversation as news. A producer
must say `historical: false` only when it knows the event arrived after its live subscription was
established.

```json
{"event_id":"provider-event-42","historical":false,"kind":"create","message":{"id":"9001","channel_id":"123","author":"Ada","author_id":"7","author_is_bot":false,"timestamp":"2026-09-18T12:00:00Z","spoken_time":"","reply_to":null,"content":"done"}}
```

Updates use `kind: "update"` with the complete current message. Deletes use
`{"event_id":"…","historical":false,"kind":"delete","channel_id":"123","message_id":"9001"}`.
They become `event: message_update` and `event: message_delete`; the page re-reads the authoritative
window while preserving its scroll position, rather than leaving a stale row or trying to patch a
derived combined row in place. Creates remain `event: message`, so existing polling producers and
older clients keep their wire contract.

SSE resume ids are event ids, not message-order cursors. Adapter ids are namespaced with `push:` on
that internal wire so even a numeric provider event id cannot be mistaken for a numeric message
cursor. If `Last-Event-ID` is still in the bounded tail, the hub replays exactly what followed it,
including edits and deletes. If an opaque id has fallen out, the server emits `event: reset` and
ends the stream; guessing a partial replay would hide a gap. The same rule applies to polling's
numeric ids: numeric comparison cannot prove that the bounded tail did not evict an intervening
message.

### Built-in Discord ingestion is bounded polling, not a Gateway connection

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

**A Gateway remains an upgrade path behind this same seam.** Everything above `src/live.rs` sees a
`LiveHub` — a channel-keyed publish/subscribe with a bounded replay tail. External adapters already
publish into it; a native Gateway implementation would do the same.

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

### The page keeps the ElevenLabs conversation socket

The server relays nothing to the vendor. It mints a signed URL and that is the end of its
involvement; the browser holds the conversation. Moving that socket server-side would turn
vibe-talk into an always-connected, **billed** conversation holder whose cost accrues while nobody
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
against the rows actually on screen, and — in either of the two speaking modes — **spoken** into a
live conversation as a `user_message`.

**`user_message`, not `contextual_update`, and that is the substance of the feature.** A
contextual update injects text into the agent's context *without consuming a turn*: the agent
silently knows a message arrived and says nothing about it until asked. That is right for a
background note, which is what this used to be, and it cannot satisfy "read new" — the point of
having the call open is to be *told*. A `user_message` consumes a turn, so the agent answers out
loud. It deliberately does not go through the page's own `sendUserMessage`, which renders what it
sends into the transcript as the reader's words: nobody said this, and putting it under "you"
would be a false record of the conversation.

**Three modes, not a toggle, and the default speaks.** `off` is silence; `gist` has the agent say
in one sentence what arrived; `full` has it read each message out word for word. They differ only
in the **task sentence** at the head of the turn, and that sentence comes *first* — ahead of the
quoted text and its `BEGIN`/`END` fence — so that no line written by a third party is ever the most
recent instruction in the turn. Both of them end with "Then stop": this is an interruption of a
conversation already in progress, and the agent's job is to say the one thing and hand the floor
back. The default is `gist`, which is a **reversal** of the earlier off-by-default and is the
owner's explicit choice; the screen says so beside the control and in its help entry, because a
setting that spends money without being asked for on the day must be legible rather than
discovered from a bill. The storage key is unchanged on purpose — a stored `"off"` is still a mode,
so an explicit earlier refusal is not overturned by the rename, while the old `"on"` falls through
to the default it now means.

**A burst is one turn.** Arrivals are held for `RELAY_COALESCE_MS` and sent together. A coding
agent posting four lines in two seconds would otherwise take four spoken turns, one after another,
each interrupting the last and leaving no gap to answer in. Both guards below are re-checked at
the flush as well as at the door, because the call can end and the reader can press Off inside
that window.

**Two controls over one value.** The bar carries **Read new**, a button that cycles
off → gist → full → off, because the strip has room for a `.bar-button` and not for a
`.bar-select`; Settings carries the same three as a named `<select>`, where there is room to say
what they mean. Both go through one function that writes the key and redraws the other, so the two
cannot disagree about which mode is in force.

Four guards on the relay, all load-bearing:

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
* **The mode.** `off` is silence. The other two both speak, and both therefore cost a turn of a
  conversation the reader is already having.
* **A live socket.** There is nowhere to send it otherwise, and queuing it for the next call would
  deliver stale news at the start of a conversation about something else.

## Conversational voice providers

The server chooses a conversational provider through `[conversation]` and returns one common
`VoiceSession` from `GET /api/v1/voice-session`. The page uses the returned `provider` as its
display name and the returned `protocol` to select the wire adapter. It does not infer a provider
from the URL or use signed-URL vocabulary in provider-neutral errors.

The deployment-managed `vibe-talk-v1` WebSocket carries JSON control frames and raw 24 kHz,
16-bit little-endian mono PCM binary frames. The server first sends
`{"type":"session_started","greeting":true|false}`. A voice client then sends `audio_start` and,
when `greeting` is true, withholds microphone frames until the first `turn_complete`. Typed input
uses `{"type":"prompt","text":"…"}`. A client switching from an open microphone to typed input
sends `audio_end`, waits for `turn_complete`, and then sends the prompt; after that response's
`turn_complete`, it sends a fresh `audio_start` before resuming microphone frames. Server output is
`transcript`, `turn_complete`, `error`, or binary PCM. `quit` ends the session.

A client may send `{"type":"interrupt"}` at any time after `session_started`. During a turn, the
server stops the response, drops that turn's remaining PCM and transcripts (including any already
queued), and answers with exactly one `{"type":"turn_complete","turn":N,"interrupted":true}`; the
next prompt on the same socket is turn N+1. When nothing is running, `interrupt` is ignored.
Either way, every accepted prompt receives exactly one `turn_complete`, so a client that waits for
it knows no later frame belongs to the interrupted turn. An `error` frame promises no following
`turn_complete`, so a client should treat it as the end of the session. A server that predates
`interrupt` answers it with an `error` frame. A client should still bound its wait for a prompt's
first output rather than trust every server to answer: read-aloud abandons a session that returns
no audio within 15 seconds of a prompt and repeats the read once on a fresh one.

This protocol is deliberately public and provider-neutral. Authentication, endpoint discovery,
and the implementation behind a deployment-managed socket belong in deployment configuration and
private operations documentation.

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
| read-aloud has neither `elevenlabs.voice_id` nor `elevenlabs.agent_id` | `503` | `elevenlabs_not_configured`, naming both — either one fixes it |
| the message to be read aloud has no text | `422` | `nothing_to_read`; no vendor call is made, so nothing is billed |
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

**Done/archive is OURS, and this is the screen that has to say so.** `#61 unread-status` settled the
direct Discord case: a bot has no ack to send and no read-state field to read. Marking something
dealt with here does **not** mark it read in the source service, and reading it there does **not**
clear it here. A deployment whose provider bridge explicitly supports upstream read marks gets a
separate **Mark read through here** action; it never changes Done/archive state.

**The store is single-tenant.** Every mark is yours, with no column saying whose. Sharing
one deployment between two people would silently merge their inboxes; that is not a
configuration this decision survives, and it would have to be revisited rather than worked around.

Three acts, each reachable from a control:

- **Done**, on the row. One message, and the answer names it, which is what makes the undo exact.
- **Clear the backlog**, above the list. Bulk and destructive, so it says how many it is about to
  take, takes two taps — the same armed idiom the dock's Clear control uses — and lapses back to
  safe after a few seconds. It sends `{through: <newest id>, limit: <the window it read with>}`
  and the **boundary is included**: the row you gave up on is part of what you gave up on, and
  that is tested at the boundary. The `limit` is not decoration: the server resolves `through`
  against a window of its own, so a boundary sent without one is resolved against the server's
  default and clears messages that were never displayed. Clearing work you never saw is the
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
  nothing is spent on messages nobody scrolls to. And when the server answers that it has **no
  summariser configured at all**, the page stops asking entirely: that is a fact about the
  deployment, not about the row, and one request settles it for the whole view rather than one
  per row — each of which carries a Discord window fetch behind it.
- **The page names the summariser it actually got**, quoting the server's own answer at the head
  of the view. Showing a model's paraphrase of somebody else's message without saying where it
  came from would be claiming a reading the reader cannot check. `web/voice.js` contains no
  summariser name of its own, which is asserted, because a constant here would outlive the
  deployment it described.

Two answers are deliberately different. `below_threshold` is the **server's** own, stricter
threshold saying the message is short enough to read as it is: settled, never asked again, the row
keeps its opening lines, and it is **not** drawn as a failure. Anything else without usable text
is a **failure**, and a failure is now visible: the row keeps the message — that is the one thing
the reader still has — and is tinted **red**, with the words `summary failed` where the summary
would have been and the reason on the row itself. The words matter as much as the colour, because
a colour is not reachable by a screen reader.

Red means "a summary was attempted for this row and it failed", whatever the reason — including a
server with no ElevenLabs credentials, which is a failure the reader is entitled to see rather
than a quiet sentence they will scroll past. Rows that were never asked about stay plain. Fifty
failures produce **one** status message with a count, not fifty; the sticky error panel is not
raised for a summary failure at all, because taking the channel away is a worse outcome than a red
row; and the deployment-level reason is stated once above the list, where a permanent fact belongs
rather than in a strip that fades after six seconds. Leaving the mode and re-entering it retries
an ordinary failure, because a failure is not a verdict — but it does not re-fire one doomed
request per row at a server that has already said it has no summariser.

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

### The transcript walks back the same way the channel does

`#48 transcript-storage` made the record of a call **durable**, and `#128 transcript-history` is
what finally lets you read it. Before it, reopening the page restored the last conversation and
stopped there: everything older was on the server, in SQLite, and unreachable from the only
interface that has it.

The restore is now a **bounded suffix of the whole record**, oldest-first within each call, and
**Earlier turns** above the list takes the next step — the same control, the same automatic step
on reaching the top, and the same anchored prepend as the channel. There was no reason for two
mechanisms, and two would have drifted.

The cursor is a **row value** over `(at_ms, conversation_id, seq)`, not a timestamp. Two calls can
have turns at the same millisecond — a restored one and a live one, or two devices — and a cursor
that compares only the clock either loses a turn at the boundary or repeats one. The server asks
for one more row than it means to return, which is how `has_more` is answered without a second
count query.

Where a call ends and the next begins is drawn as a **seam**, because an unbroken list of turns is
itself a claim that it is all one conversation, and across a walk back through weeks of calls that
claim is false more often than it is true.

### Finding something in what is on screen

A magnifying glass in the corner of the header opens a field, and the field filters **the messages
already loaded** — the channel and the voice transcript alike, because they are one switch apart
and a filter that came off when you looked at the other list would be worse than none.

**Every term has to match**, and matching is substring rather than whole-word: a second word is
typed to narrow, and "runner" has to find "runners". **Double quotes group words into one term**,
so `"mac runner"` finds the phrase and not the two words scattered through a paragraph; an
unterminated quote groups to the end of what has been typed, which is the state the field is in
for every keystroke between the two quotes.

The count beside the field says **"2 of 13 loaded"**, and the last word is load-bearing. Since the
section above, what is on screen is a bounded suffix of a longer record, so "no matches" and
"nobody ever said that" are different answers and a bare number would let you conclude the second.
For the same reason, a search that matches nothing says so **where the messages were** rather than
only in the corner: every row is still in the list, hidden by a class of its own, so neither pane's
empty state fires and the reader would otherwise get a blank screen.

A class of its own, and not the `hidden` attribute, because `hidden` is where this page already
records the *other* reasons a row is off screen — To do mode, dismissed, which view is up. A filter
that borrowed it would put archived rows back on screen when the search was cleared.

Rules the page drew itself — the seam between two calls, the date rules — **go away with the rows
they were explaining**. A seam says the agent below it never heard the words above it, and left
standing over a filtered list it says that about two rows that are no longer adjacent.

### Choosing the channel is a control, not a line of the scrollback

The picker used to be a row at the **top of the scrolling region**, above the log. That is the
whole of the defect that was reported against it: the control for choosing *what you are reading*
scrolled away the moment you read anything, and getting back to it meant scrolling a channel to
its beginning.

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
names what the channel view is reading; Type and Prompts go out through `sendUserMessage`, which
draws the line into the **transcript** and needs a live call, so on the channel view they are
controls whose whole result appears on a screen you are not looking at; and Read new decides what
happens to an arriving message *during a call*, so it belongs where the call is. A member with no
line in the table is hidden everywhere and the page suite names it; there is no default, because a
default is how a control ends up on a view nobody chose for it.

That is a layout decision as much as a semantic one, and it is **measured**: the bar is one strip
on a 375px phone, six controls do not fit on it, and the pack scrolls — so a control pushed past
the pack's right edge is exactly as unreachable as the picker used to be. "The bar fits on a
375px phone" costs whatever `renderControlBar` really leaves visible against widths read
out of web/voice.css, on each view and in text mode, and the same test proves it is not vacuous by
showing that the strip as it stood before the prompts tray — every canned prompt a member in its
own right — overflows by about 55px. That arithmetic is also **why the tray came first**: Read new
is on the strip at all because collapsing the prompts gave a slot back, and it is a cycling button
rather than a `<select>` because a `.bar-select` needs 5rem the call view does not have.

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
`1532416065114607829`, and whatever `label` was written in the configuration file — which is often
something like `build noise`, chosen when there was one channel and nobody was addressing
it out loud. The motivation is the next feature rather than this one: with several channels
configured, *"ask the build channel"* has to resolve to something, and a short name you chose
yourself is the only candidate.

So you can **give a channel a name of your own, in this app**, from Settings. The alias wins
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
name the channel the way you do, so the name you say out loud is the name the model was
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

This came out of a real report — the channel found hours out of date: *"especially when I swipe up
on this view and it shows me something very stale."* The staleness itself was already fixed — the view re-reads on
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
mid-call and iOS suspends the page**, the socket dies, and the page used to greet you on your
return with a red panel saying the connection to the voice agent had FAILED. Nothing had failed.
You switched apps.

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
floor, which on a real handset put the first word of the status line into the corner curvature and
ate it. Top — under the title, where it used to be — is a setting, kept in `localStorage` under
`vibe-talk.voice.bar-placement` and offered in Settings. There is **one** bar and two mount points,
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

**They are in a tray, behind one `Prompts` button**, and the reason is arithmetic rather than
taste: the bar is a single 375px row that was priced to the edge, each prompt cost it a member,
and the list is meant to grow — custom prompts are the next thing. One opener costs one slot
however many prompts it holds. The tray is a **sibling of the pack and a child of the bar**: of
the bar because the bar is what moves between the header mount and the dock mount, and out of the
pack because the pack scrolls sideways and a menu that can scroll out from under its own opener is
not a menu. It is absolutely positioned, so it takes no width from the strip at all, opens upward
in the dock and downward in the header, and wraps rather than scrolling. The budget test reads
"out of flow" **out of the stylesheet** rather than from a list, so a tray that stopped being
absolute starts costing the strip real width on the edit that changed it.

It opens **at idle**, deliberately. With no call the prompts grey out, and a reader has to be able
to see what they are in order to learn why they are dead.

They differ in **weight** — Blockers spends coding-agent work on one tap — and that difference
used to be drawn, in the warm tone an armed Clear uses. **It no longer is**, at the owner's
request: on a desktop screen the warm outline did not read as "this one is expensive", it read as
a warning symbol, as though the button were reporting a fault, and a control that looks broken is
worse than one whose cost is only in its tooltip. The weight now lives in the `title` and in the
Settings text. Neither button asks twice — the issue did not ask for a confirm step, so adding one
is a decision for somebody to make out loud rather than drift into, and the suite records its
absence so it is not mistaken for an oversight.

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
the obvious fallback, *since you last spoke*, is not computable either: the digest has no
identity for you, your own replies through this bridge are posted **as the bot**, the digest drops
the bot/human flag, and the only author signal is a display name anyone can set to anything. A button
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

`#73 mute-is-invisible`, and it came out of a real complaint: during a long mute the agent "gets
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
nothing, and do not ask whether they are still there" — when it is told the microphone is muted. It
does **not** name the `skip_turn` tool, which is a separate native vendor feature described further
down; nothing here wires the two together.

**None of this makes a mute cheaper.** Billing continues while muted; the vendor discounts silent
periods but does not stop the meter. This makes a long mute quieter, not free.
### A typed conversation is a different conversation, not a call with the switches thrown

Because a text-only mode is settled *at initiation*, it can be **chosen** there — and that is what
**Type** does when it is pressed with nothing open. It answers a real report: reaching a text
interface used to mean starting a voice call, muting it, and silencing it. Two of
those three controls exist to manage a microphone you did not want open, and mute deliberately does
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
| Server URL | `https://<your-public-hostname>/mcp` — the path **must** be `/mcp` |
| Transport | **Streamable HTTP** |
| Authentication | header `Authorization`, value `Bearer <your vibe-talk read token>` |
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
cannot verify it.** Nothing here can tell whether you configured approval correctly, or
whether it was later changed. What this server enforces is the scope split and the allowlist:
a read token cannot post, and no token can reach a channel outside the configuration. Do not
mistake the approval prompt for a guarantee we implement.

Verify the public endpoint before pointing an agent at it — same script, public URL:

```sh
scripts/verify-deployment.sh --url https://<your-public-hostname> --channel <snowflake>
```

The read token's `tools/list` must NOT contain `post_reply`; check 2 is the one that asserts it.
If it fails there, something is wrong with the deployment, not with the agent, and no amount of
dashboard configuration will fix it.

## Exposing it: one worked example

**This is optional, and this vendor is not special.** See **From zero → Expose it** above for what
is actually required (a URL a browser can load; HTTPS if you want the microphone) and for the
other ways to get there — an SSH reverse tunnel, a reverse proxy with a certificate, a cloud load
balancer, an overlay network, or no exposure at all. One example is written out in full here
because a complete recipe is worth more than six sketches; pick a different one freely.

What every route shares: the server speaks plain HTTP and holds a bot token, so it must never be
reachable directly from the internet. Something in front of it terminates TLS.

The example is a Cloudflare Tunnel, which gives it a public HTTPS hostname with **no inbound port
open** on the host: the `cloudflared` daemon dials out and Cloudflare terminates TLS. You need a
domain on Cloudflare; the hostname can be a subdomain of one you already have.

```sh
# once, on the host
cloudflared tunnel login                                     # browser; pick the zone
cloudflared tunnel create vibe-talk                          # prints a UUID and a JSON path
cloudflared tunnel route dns vibe-talk vibe-talk.<your-domain>
```

`tunnel create` prints the two values the next file needs: the **tunnel UUID** and the path to the
**credentials JSON** it just wrote (normally `~/.cloudflared/<uuid>.json`).

`~/.cloudflared/config.yml`:

```yaml
tunnel: 6f2a1c30-1111-2222-3333-444455556666
credentials-file: /home/<you>/.cloudflared/6f2a1c30-1111-2222-3333-444455556666.json

ingress:
  - hostname: vibe-talk.<your-domain>
    service: http://127.0.0.1:8080
  # Required, and must be last: cloudflared refuses to start without a catch-all rule.
  - service: http_status:404
```

`tunnel:` also accepts the tunnel's name, but the UUID is what `create` handed you and it cannot be
ambiguous.

```sh
cloudflared tunnel run vibe-talk        # foreground first, so you can watch it connect
```

Verify the public hostname with the same deployment check you ran locally:

```sh
scripts/verify-deployment.sh --url https://vibe-talk.<your-domain> --channel <snowflake>
```

Then install it so it survives a reboot:

```sh
sudo cloudflared service install
sudo systemctl status cloudflared
```

Bind the server to loopback when you do this — `VIBE_TALK_BIND=127.0.0.1:8080` — so the only path
in is the tunnel. With Podman, publish to loopback: `-p 127.0.0.1:8080:8080`. That last rule is
the one part of this section that transfers unchanged to every other way in.

### If you chose Cloudflare: do not put Cloudflare Access in front of this

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

**The general rule, whichever way in you chose: no browser-shaped identity gate in front of
`/mcp`.** Any SSO, OAuth login wall, or "sign in to continue" interstitial — Cloudflare Access,
an nginx `auth_request` against an IdP, an application load balancer's OIDC action — assumes a
human who can be redirected. A hosted voice agent cannot be. If you want a second fence there, it
has to be one that a machine can satisfy with a header it was configured with.

**Transport is not authorization.** A tunnel, a proxy or a load balancer gives you TLS and hides
the host's address; none of them decides who may call. This server's bearer tokens do that, and
that is the part this codebase tests.

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
  Two things follow, and both are observations from use rather than claims of ours: **billing continues
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
Wrapping as-is would therefore leave an unauthenticated public endpoint that can post to your
Discord. That is disqualifying, and it means "wrapping" would really mean writing both a
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
your channels, and it is designed on the assumption that it may become publicly reachable — which
makes it, not the voice agent, the real security boundary of the whole design.

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
* **Stored transcripts are not encrypted at rest.** Once `storage.path` is set, your own
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
cd vibe-talk
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

# the deployment check, against an in-memory Discord — no bot token needed
cargo run -- --config vibe-talk.toml --fake-discord &
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
`tests/voice_page.rs`, which FAILS rather than skips when Node is absent. The fake is not a rubber stamp — it shares the real client's
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
theoretical: a single photograph of a real phone showed three defects the whole suite had
passed over, because a clipped paragraph and a control that reads as active on a dead call are
layout facts and a fixture with no layout engine has no opinion about them.

```sh
scripts/run.sh --screenshots
```

Photographs the `/voice` page in the thirty-six states that look different — signed out, idle, live
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
with, the bulk clear saying how many it is about to take, the channel filtered by a quoted phrase
typed into the search glass, and the same summary mode when the
summariser FAILS — the row red, saying so in words, with the message still under it — at four
viewports: a tall phone, a
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

**Dark is the default, and that is not a preference.** This page is used on a phone in a car,
where the scheme is usually dark; the first run of this harness captured nothing but light frames,
because light is the browser-automation default — so every image was real and every one was of a
page nobody actually looks at. Contrast between the two speakers is exactly what does not survive
the swap. `--theme light` or `--theme both` when you want
the other one.

It costs nothing and needs no hardware. For the **live-call** states the conversation WebSocket is
replaced before any page script runs, and the mint request to `/api/v1/signed-url` is answered
locally, so those are reached without ElevenLabs being contacted. The microphone is Chromium's own
fake capture device, so `getUserMedia`, the AudioContext and the real capture graph all still run.

For the two **summary** states there is no page-local stub at all, and there cannot be: summaries
come from a conversation with the ElevenLabs agent and from nothing else, so a harness with no
vendor behind it cannot photograph a summarised channel. `scripts/run.sh --screenshots` therefore
starts `vibe-talk-mock-elevenlabs` — this repository's own loopback vendor, a real mint endpoint
and a real conversation socket — on the two ports above the server's, points the throwaway
config's `elevenlabs.api_base` at it, and passes the harness `--mock-control` so the states can
drive it. One asks for a working summariser and photographs the summarised channel; the other
wedges the same mock so it answers nothing, and photographs the **red** failed row. Those two
frames are therefore evidence about the real socket client and the real cache, not about a stub.
That is most of the remaining half of `#57 elevenlabs-mock`; the live-call states are what is
left. Both processes are stopped by the same trap, including on failure.

It stands up its own throwaway native server with `--fake-discord` on port 18091 and stops it
again, including on failure — it refuses port 8080 by name, because that is the live deployment,
and it refuses a `--port` that would put the mock on 8080 or on the CI container's 18081.

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

`scripts/screenshots.py --self-test` runs 52 controls for those checks offline, with no browser and
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
src/summary.rs        the extractive digest/preview helper (NOT a summariser: it also flattens
                      the agent's own reply)
src/retrieval.rs      semantic random access, behind the Ranker trait
src/untrusted.rs      the data-not-instructions boundary
src/ops.rs            the operations both front doors share: allowlist, fetch, transform
src/live.rs           bounded polling and adapter push, per-channel fan-out, replay, and SSE bodies
src/replay.rs         rebuilding continuity across a hang-up: the preamble, the budget, the fence
src/probe.rs          the startup channel reachability probe and its failure taxonomy
src/diagnostics.rs    those same checks on demand, structured, redacted, and time-bounded
src/elevenlabs/       the SignedUrlProvider trait, the live client, the in-memory fake
src/elevenlabs/mock/  the loopback vendor: a real mint endpoint, a real WebSocket, a real MCP client
src/bin/mock_elevenlabs.rs     that mock as a process, for a browser or the smoke script
src/mcp/mod.rs        the tool manifest and per-tool approval policy
src/mcp/protocol.rs   JSON-RPC 2.0 and the MCP method set
src/mcp/transport.rs  the Streamable HTTP endpoint at /mcp
src/store/            the StateStore trait, the SQLite backend, the fake, the refusing one
src/summarize/        the Summarizer trait, the ElevenLabs agent summariser, the counting fake,
                      the cache key
src/agent_backend.rs  the slow-path seam
src/http/             router, handlers, and the access-log middleware
web/                  the phone app and the /voice page (plain HTML/CSS/JS, no framework, no build step)
tests/js/             the /voice page's own suite, run from cargo test via tests/voice_page.rs
scripts/verify-deployment.sh   the one-command deployment check, local or public
scripts/smoke-agent.py         the manual, billed check that the AGENT really calls us
scripts/screenshots.py         photographs /voice in every state, so an agent can SEE the page
QUICKSTART.md         the six-step setup path, start to first conversation
```

### Why your own messages need a setting

Two accounts carry your words in a channel: the one this bridge posts as, and the one you type
into Discord with yourself. The channel view draws both as yours.

The first is free. A Discord bot token's first segment IS the bot's user id, so the server reads it
out of its own token and tells the page before the first message is drawn.

**The second cannot be derived from anything this server holds.** A bot's account has no
relationship to the human reading the channel — the token says what the bot is, not who you are.
So it is `discord.owner_user_id`, and until you set it your own messages come through as somebody
else's.

You can also set it without a restart: **Settings lists every account it has seen** and lets you
say which is which. That is the faster fix, and it is per-browser; the configuration setting is for
a deployment that would rather state it once.

## Running the tests

```bash
cd vibe-talk && make validate        # fmt, clippy/mypy, suites, DAG and Android evaluator controls
cd vibe-talk && make page            # just the page suites — seconds
cd vibe-talk && make validate-boxed  # adds PWA/offline-cache/screenshot browser checks under dagrun
```

`make validate` needs Rust, Node, and Python with mypy, PyYAML, Playwright, and websockets. The
Python packages type-check the operator and browser harness scripts; no browser binary, phone, or
external service is needed by the ordinary gate. The boxed gate additionally needs Chromium for
its PWA, offline-cache, and screenshot checks.

**Prefer `validate-boxed` locally.** It runs the same suites through `dagrun`, which gives each
step a wall and CPU timeout, a memory cap, and process-tree teardown inside a cgroup. That last
part is not a nicety: the screenshot harness starts a real server and a real browser, and the
teardown is setsid-proof — they are reaped with the step whether or not the script's own trap
fired. One that was not once ran for six days holding a port. The graph also caps `browser: 1`,
so two Chromiums can never run at once however many steps ask for one.

The graph is `vibe-talk/tests.dag.yaml`. `make dag-check` loads it without running anything.

## Known gaps

Beyond the security list above:

* **`GET /api/v1/diagnostics` has met no real vendor.** Both halves of it — the Discord checks and
  the ElevenLabs checks — have been exercised only against in-memory fakes. The classification it
  reports is the startup probe's, which is in exactly the same position; what a live vendor
  actually returns for each of those cases is still unobserved.
* `resolve` only searches the window it just fetched (default 50, max 100 messages). Older messages
  are unreachable; there is no pagination and no store.
* Ranking is lexical. It matches words, not meaning, so a paraphrase with no shared words will miss.
  The `Ranker` trait is the replacement point.
* **The only summariser has never met live ElevenLabs, and it is now the default path.** Every
  per-message summary is a real conversation with the configured agent; the extractive truncating
  fallback is gone. What has been exercised is the socket client against this repository's own
  mock, written from the same reading of the vendor's SDK — so the two agree with each other and
  not yet with the vendor. Until somebody runs it for real, the summary feature is unproven on
  every deployment, not just on the ones that opted in.
* **A deployment with no ElevenLabs credentials shows every long row as failed.** It starts, it
  warns, and it says so on the page — but the cache stays empty forever and there is no reduced
  service to fall back on. `summaries.model` is still recorded in the cache key and read by
  nothing, which is the last vestige of a selector that no longer exists.
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
* The web app has no service worker and no offline mode. It keeps a bounded snapshot of recent
  channel rows in `localStorage` (see "Saved messages on this device"), but the page itself
  cannot load without the server.
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
* **A direct Discord bot still cannot reach Discord's read state.** Discord shares it with
  clients, not with bots — no ack route, no read-state field on the channel object, no
  `read_state` in the gateway READY payload. The evidence and code anchors are in
  `ai_docs/UNREAD_STATUS_20260819.md`. A compatible provider bridge may expose the separately
  configured `/upstream-read` capability, but that does not turn vibe-talk's own `/read` record or
  its Done/archive state into provider state.

## License

MIT, as with the rest of the repository.
