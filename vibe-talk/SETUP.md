# vibe-talk setup: install, connect, and pair your phone

This guide takes a new user from nothing to reading and replying to agent chat on a phone. Pick
**one** connection route; you do not need the others. [`QUICKSTART.md`](QUICKSTART.md) is the
Discord-and-voice walkthrough with exact vendor steps, and [`README.md`](README.md) is the full
reference. This page is the map between them.

> **Agent-assisted setup.** If you use the agent-utils suite, its setup skill is wizard-style:
> you point an agent at the repository and it asks, in small batches with a recommended default
> for each question, which optional tools you want. `vibe-talk` is one of those options. Choosing
> it does not install vibe-talk by itself: the skill hands you this guide and the quickstart,
> because vibe-talk is a separately deployed service with its own host, tokens, and TLS front
> end. Answer its questions, then follow the steps below on the machine that will run the server.

## 1. Install on a machine

You need one machine that stays on while you use the phone app: a home desktop, a small cloud
host, or a work machine you are allowed to run a service on. It needs outbound access to your
chat provider. It does not need an open inbound port for most routes below.

**Prerequisites**

- Linux or macOS with `git`, and either a Rust toolchain (`cargo`, from <https://rustup.rs>) for
  a native build, or a container runtime (`podman` or `docker`) for the container build.
- A chat credential: for Discord, a dedicated bot token (quickstart step 1); for Slack or a
  compatible bridge, that provider's token. Use a dedicated credential, not the one your coding
  agents post with.
- Two tokens you generate yourself: `openssl rand -base64 33`, twice. They must differ and each
  be at least 24 characters. The read token can read; the write token can also post.

**Build**

```sh
git clone https://github.com/rrnewton/agent-utils.git
cd agent-utils/vibe-talk
cargo build --release --locked   # binary: target/release/vibe-talk
# or: podman build -t vibe-talk:v0 -f Containerfile .
```

**Configure**

Copy `vibe-talk.example.toml` to `vibe-talk.toml` outside any synced or committed directory, and
fill in the bot token, the two tokens above, your channels, and a durable storage path. The
smallest useful addition for a first run with no voice vendor is:

```toml
[storage]
path = "/var/lib/vibe-talk/vibe-talk.sqlite3"   # absolute; create the directory first

[read_aloud]
backend = "browser"   # read aloud with the phone's own voice; no vendor account needed
```

Keep the server bound to loopback until you choose a route: `bind = "127.0.0.1:8080"` under
`[server]`. Secrets may also come from the environment (`VIBE_TALK_READ_TOKEN` and friends win
over the file); never commit a filled-in copy.

**Check, then start**

```sh
vibe-talk doctor --config vibe-talk.toml   # one line per check, with the fix for each
vibe-talk --config vibe-talk.toml          # or: scripts/run.sh for the container launcher
```

`doctor` makes no network call and prints no token. Fix every `FAIL` line; read the `WARN`
lines, because each one names a feature that will otherwise surprise you later (no public
address for pairing, plain-HTTP exposure, no writable channel, live updates off, voice not
configured). Then open <http://127.0.0.1:8080/healthz>: it should answer `ok`. The startup
channel probe reads one message per configured channel and refuses to start, naming the
channel and the fix, when the bot cannot read it.

## 2. Choose how the browser reaches the server

vibe-talk serves plain HTTP and terminates no TLS itself. The browser needs a secure context
(HTTPS, or `localhost`) for the microphone and for installing the app, so the route you choose
decides which features work. The server's own read/write tokens decide who may call it on every
route; transport alone is never the authorization.

| Route | Reach it from | Microphone and install | Opens an inbound port? |
|---|---|---|---|
| Same machine | This machine only | Yes, `localhost` counts as secure | No |
| LAN | Devices on your home network | No over plain HTTP; yes behind local HTTPS | No (LAN only) |
| SSH port forward | Any machine you can SSH from | Yes, through `localhost` | No |
| Cloudflare Tunnel | Anywhere | Yes | No |
| Tailscale | Your devices, or anywhere with Funnel | Yes | No |
| Corporate web proxy | Machines your organisation allows | Usually yes | No |

### Route A: same machine (localhost)

Open <http://127.0.0.1:8080/>. Nothing to configure. This is the fastest way to see the app,
and with `--fake-discord` it needs no chat credential at all. A phone cannot use this route.

### Route B: LAN

Bind or publish on the LAN (for the container, `-p 8080:8080` instead of
`-p 127.0.0.1:8080:8080`), then open `http://<machine-name-or-LAN-address>:8080/` from the
phone on the same Wi-Fi. Set `[server] public_url` to that address so pairing links point at
the right place.

*Security and trade-offs.* Anyone on the LAN can reach the login page; the write token is the
only fence, so treat the LAN as trusted or choose another route. Plain HTTP gives the browser
no microphone and no installable app. This route is a good permanent home setup only inside a
network you trust, and the right first test everywhere.

### Route C: SSH port forward

Keep the server on loopback and forward it to the machine in front of you:

```sh
ssh -L 8080:127.0.0.1:8080 <you>@<server-host>
```

Then open <http://127.0.0.1:8080/> in that machine's browser. No port opens, SSH is the
encryption and the login, and `localhost` is a secure context, so the microphone works.
*Trade-off:* the forward lasts while the SSH session does, and a phone cannot hold this route
without an SSH client app and a background tunnel. Best for a desktop browser; use a tunnel,
Tailscale, or the proxy route for a phone.

### Route D: Cloudflare Tunnel (named tunnel)

`cloudflared` dials out to Cloudflare, which terminates HTTPS and forwards to the local port.
No inbound port opens and no cloud host is needed. You need a domain managed by Cloudflare.

```sh
cloudflared tunnel login
cloudflared tunnel create vibe-talk          # note the tunnel UUID and credentials file
cloudflared tunnel route dns vibe-talk vibe.example.com
```

Write `~/.cloudflared/config.yml` with your UUID, credentials path, and hostname, ending with
the required catch-all:

```yaml
tunnel: <tunnel-uuid>
credentials-file: /home/you/.cloudflared/<tunnel-uuid>.json
ingress:
  - hostname: vibe.example.com
    service: http://127.0.0.1:8080
  - service: http_status:404
```

Run `cloudflared tunnel run vibe-talk` in the foreground once, then install it as a service
(`sudo cloudflared service install`). Set `[server] public_url = "https://vibe.example.com"`,
restart vibe-talk, and verify with `scripts/verify-deployment.sh --url https://vibe.example.com
--channel <channel-id>`.

*Security and trade-offs.* Cloudflare sees the traffic in transit and the hostname is public:
the read/write tokens are what keep strangers out, so keep them long and private. Cloudflare
Access may fence the browser pages for named people, but never fence `/mcp` with a browser
login: a voice agent calls it machine-to-machine and will be bounced to a login page. If you
want Access on the whole hostname, add a service token for the agent and send its
`CF-Access-Client-Id` and `CF-Access-Client-Secret` headers from the agent's configuration.
The tunnel depends on Cloudflare and on DNS; when either is down, the LAN and SSH routes still
work.

### Route E: Tailscale

Install Tailscale on the server and on the phone, sign both into the same tailnet, then either
open `http://<machine-name>:8080/` over the tailnet, or publish it inside the tailnet with
HTTPS:

```sh
tailscale serve --bg 8080   # serves https://<machine>.<tailnet>.ts.net to your tailnet only
```

Set `[server] public_url` to the address you actually open. *Trade-offs:* `tailscale serve`
gives real HTTPS (microphone and install work) to your own devices and nobody else, which is
the smallest public surface of any remote route. Tailscale Funnel (`tailscale funnel 8080`)
instead publishes the same port to the whole internet: only choose it when a machine outside
your tailnet, such as a hosted voice agent, must call `/mcp`, and know the tokens are then the
only fence. Both depend on Tailscale's control plane for new connections.

### Route F: a corporate dev-server web proxy (generic)

Many organisations run development machines behind a web proxy: you open an organisation HTTPS
address in a browser, sign in with your work account, and the proxy forwards the request to a
port on your dev machine. If yours offers this, run vibe-talk on loopback and follow your
organisation's own instructions to map its proxy address to `127.0.0.1:8080` on that machine.
Set `[server] public_url` to the proxy address browsers actually open.

*Security and trade-offs.* Access rides on your work login, which is a real second fence, and
no inbound port opens. Ask three questions before relying on it: does the proxy pass
WebSockets and long-lived streaming responses (the live view and voice need them), does it add
its own login redirect that would bounce a machine caller of `/mcp`, and may personal phone
traffic use it under your organisation's rules? A proxy that buffers or time-limits streams
gives a page that loads but never updates. Never name the proxy, a machine, or an account in a
public configuration file or issue.

Whichever route you chose, run the deployment check against the public address as well as
against loopback. If loopback passes and the public address fails, the fault is in the route,
not in vibe-talk:

```sh
scripts/verify-deployment.sh --url <the-address-a-browser-opens> --channel <channel-id>
```

## 3. Install the phone app and pair it

Serve the app over HTTPS (any route above except plain-HTTP LAN), then:

- **Android (Chrome):** open the app's address, sign in once, then use the browser menu's
  **Install app**. The installed app opens in its own window.
- **iOS (Safari):** open the address, then **Share → Add to Home Screen**. On iOS this is a
  home-screen shortcut in the browser rather than a separate installed app, which keeps
  microphone handling on the safer browser path. For an iPhone home-screen app, open the
  installed app and paste an unused pairing link into **Pairing link or code** on its sign-in
  screen, then tap **Pair this app**.

**Pairing without typing the token.** On the server's host, with `[server] public_url` set to
the address the phone opens:

```sh
vibe-talk pair-link   # add --config PATH if the server was started with one
```

It prints one link, good once for five minutes. Send it to the person through a conversation
only they read, and tell them: open it on the phone and tap **Sign this device in**. Whoever
opens it first is signed in with the write token. If the phone says the link expired or was
already used, run `pair-link` again and send the new link; if they had not opened it, someone
else did, and the write token should be rotated. The full protocol and its limits are under
"Pairing a phone" in the README.

**Your first channel.** The channels in the configuration are the channels in the app; there
is no in-app step that creates the first one. To add one, add a `[[channels]]` entry (or extend
`VIBE_TALK_CHANNELS`), restart, and run `vibe-talk doctor` again. If the app opens with no
channels, the configuration it loaded has none that the startup probe could read.

## 4. When setup goes wrong

- **`doctor` says no durable storage.** Set `[storage] path` as above. Without it the server
  starts and `/healthz` passes, but the channel views answer 503 by design.
- **The server refuses to start and names a channel.** The bot is in the chat server but was
  never added to that private channel, the channel id is wrong, or the token is bad. The probe
  message says which; fix that one thing and start again.
- **Messages arrive blank.** For Discord, turn on **MESSAGE CONTENT INTENT** for the bot. The
  connection otherwise works and every digest line is empty, with no error anywhere.
- **The page loads but the microphone does nothing.** The address is plain HTTP. Use
  `localhost`, an SSH forward, or an HTTPS route.
- **The pairing link opens the wrong host or `127.0.0.1`.** `public_url` names loopback or is
  unset. Set it to the address the phone opens, or pass `--base-url` to `pair-link`.
- **A hosted voice agent cannot connect.** A browser login (an identity-provider wall or
  Cloudflare Access without a service token) is fencing `/mcp`. Remove it from that path or
  give the agent a service token; transport is not authorization, and the bearer token is the
  fence vibe-talk tests.
