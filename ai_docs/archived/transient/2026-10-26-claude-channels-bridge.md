# Coordinator chat bridge over Claude Code channels: scoping note

Status: proposal, not implemented. Written 2026-10-26 for #556 chat-delivery-latency, at the
owner's request, after the latency fix. Facts about Claude Code channels come from its public
documentation (code.claude.com/docs/en/channels and channels-reference, fetched 2026-10-26); they
were **not** verified against the Claude Code build on this host, whose bundle could not be
inspected.

## Why consider it

`agentctl chat run` reaches a Claude Code coordinator the way a person would: it types the
request into the pane's composer, proves the submission from the screen, and scrapes replies
written between two marker lines. Everything that went wrong on 2026-10-26 follows from that:

- Typing must be exclusive, so every sender to an agent takes its lifecycle lock until delivery
  is confirmed. Chat requests queued behind agent messages for up to 56 minutes (#556).
- Proving a submission and ruling out a misroute means reading the target pane and every other
  agent's pane before and after typing. On a loaded terminal server that cost 1-2.5 minutes per
  delivery until the reads were capped and made concurrent.
- The bridge depends on how Claude Code draws its composer, its queued-message hint, shell mode
  (#559 claude-shell-mode-prompt), its full-screen renderer, and its scroll pill (#397).
- Replies are found by reading the screen or the session transcript, and a reply printed in
  thinking or redrawn away is lost unless the agent runs `agentctl chat reply`.

A channel replaces both directions with a program-to-program protocol.

## What a channel is

A channel is an MCP server that Claude Code spawns over stdio and that declares the
`experimental["claude/channel"]` capability. It pushes events with the notification
`notifications/claude/channel` (`content`, plus string `meta` attributes); each arrives in the
model's context as `<channel source="NAME" key="value">content</channel>` and is shown in the
terminal as a one-line summary. A two-way channel also offers ordinary MCP tools, such as `reply`,
and an `instructions` string that tells the model when to call them. An optional
`claude/channel/permission` capability relays tool-approval prompts to the channel.

Delivery semantics that matter here:

- Notifications are not acknowledged: the send completes when written to stdio.
- Events queue in the session and are handled in order; several that arrive while the model is
  busy are delivered together **on the next turn**.
- Sessions opt in per start: `--channels plugin:NAME@MARKETPLACE` for allowlisted plugins, or
  `--dangerously-load-development-channels server:NAME` for anything else, which shows a
  confirmation dialog at startup and is ignored in non-interactive (`-p`) sessions.
- Channels are a research preview: the flag syntax and protocol may change.
- **They need Anthropic authentication (claude.ai or a Console API key) and are not available on
  Bedrock, Google Cloud's Agent Platform or Foundry.** Organizations with managed settings also
  need `channelsEnabled: true`, and `allowedChannelPlugins` replaces the default allowlist.

The last point decides feasibility before anything else: a coordinator whose Claude Code
authenticates through a cloud provider or a gateway in front of one may not get channels at all.
First step, before any code: start a test session with a trivial development channel and see
whether the startup notice registers it or says it is blocked.

## What agentctl would provide

agentctl already serves MCP over stdio: `agentctl mcp` (#545 rust-primary-agentctl-chat-readers)
exposes the CLI's operations as tools, each run through the same executable. The channel is a
mode of that server, say `agentctl mcp --channel --bridge-state DIR`, that also declares
`experimental["claude/channel"]`, runs a notification loop, and adds the chat tools below. Claude
Code starts it (registered in the coordinator's `.mcp.json` or user config and named on the
coordinator's launch line). It is a thin relay; `chat run` stays the long-lived service that owns
the provider subscription, admission, durable request records and outbound sends.

- **Inbound.** The relay watches the bridge state for admitted requests not yet handed to this
  session and emits one notification per request, with `meta` carrying the request key, reply ID,
  thread, sender and sent time, and `content` the message text plus the short context the prompt
  carries today. It records the hand-over in the request record. With no acknowledgement, the
  record means "written to the session", not "read by the model".
- **Outbound.** Tools: `reply(request, text)`, the same path as `agentctl chat reply` (stored
  durably, idempotent per reply ordinal, wakes the service to send); `thread(request, last)` to
  read earlier messages; optionally `close(request)`. No marker lines, no screen capture.
- **Restart.** The relay lives and dies with the Claude Code session. On connect it hands over
  every request that is admitted and unreplied, so a restarted coordinator sees what it missed;
  the request key in `meta` lets the model recognise a repeat, and `reply` stays idempotent.
- **Permission relay (optional).** Forward approval prompts to the owner's thread and accept a
  verdict back: the remote approval that today needs someone at the pane.

## What changes in `agentctl chat`

`chat_runtime` already separates delivery behind the `CoordinatorDelivery` trait (message state,
submit, drain, screen). A channel transport is a second implementation:

| Today (pane) | With a channel |
|---|---|
| queue + lifecycle lock + readiness wait | write the request to the relay's hand-over set |
| composer proof, misroute read-back, peer snapshots | none: stdio reaches exactly one session |
| reply capture from screen and transcript, alias gate (`guard_typing`, `prompt_screen`) | replies only through the `reply` tool |
| reply reminders read panes and transcripts | reminders become notifications ("request N is unanswered") |
| stale-composer clearing, receipt reaction on a verified submission | not needed; the receipt reaction moves to the hand-over or to the first reply |

Configuration gains `transport = "pane" | "channel"`; status reports which one is active and,
for a channel, whether a relay is connected. The pane path must stay for every other harness and
as a fallback.

## What is lost

- **Mid-turn delivery.** Claude Code shows input typed into a working pane inside the running
  turn, alongside the next tool result. Channel events wait for the next turn, so a request sent
  during a 30-minute turn waits 30 minutes, where today's path with `--deliver-while-working`
  does not. This needs measuring before any switch; it may favour keeping the pane path for
  urgent messages.
- **Proof that the model saw a request.** A typed prompt is proven on screen; a notification is
  only written to a pipe. The first `reply` call, or an explicit acknowledgement tool, is the only
  evidence.
- **The owner's view.** Today the full request text is in the terminal; with a channel the TUI
  shows a one-line summary. Replies printed between marker lines also appeared in the terminal;
  tool-call replies show as tool calls.
- **Portability and stability.** Claude Code only, Anthropic authentication only, research
  preview, an interactive confirmation for development channels on every start (agentctl
  `revive` and `restore` would have to answer it), and plugin packaging plus an allowlist entry
  to avoid that.
- **Restart coupling.** The relay restarts with the session, so hand-over state must be durable
  in the bridge, which it already is.

## Muse and Codex

Neither has Claude Code's channel contract.

- **Codex** offers programmatic protocols (`codex app-server`, `codex mcp-server`) that drive a
  session as a program, but they replace the shared TUI rather than add a channel to it. A Codex
  coordinator would keep the pane transport, or run headless under an app-server client that
  agentctl provides, giving up the owner's live pane.
- **Muse** takes follow-ups through its verified editor path (#505 agentctl-muse-followup); no
  push channel is known. Keep the pane transport; revisit if Muse gains MCP notifications.

So the bridge needs a transport choice per coordinator, not a replacement: channel for a Claude
Code coordinator where channels are enabled, pane everywhere else.

## Suggested order

1. Feasibility probe on this host: a development channel in a throwaway Claude Code session (no
   live coordinator), check registration, event rendering, next-turn delivery, `reply` tool calls.
2. Measure mid-turn latency against `--deliver-while-working` on the same test session.
3. If both are acceptable: implement the relay and the `CoordinatorDelivery` channel transport
   behind `transport = "channel"`, with fake-session tests (an MCP client that plays Claude Code).
4. Trial on a second coordinator before the owner's.
