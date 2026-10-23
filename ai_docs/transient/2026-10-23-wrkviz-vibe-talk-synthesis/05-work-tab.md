# Appendix 5: The Work tab

> **Update 2026-10-23 (`#455 vibe-talk-next-plan`).** Part of the master plan,
> [`../2026-10-23-vibe-talk-next/README.md`](../2026-10-23-vibe-talk-next/README.md), which links
> here for the detail of the Work tab (milestone 0.6.0). Terms follow its
> [terminology](../2026-10-23-vibe-talk-next/TERMINOLOGY.md).

Part of `#449 wrkviz-vibe-talk-synthesis`. A third view in the vibe-talk web app, beside Voice and
Channel: the wrkviz timeline for the current channel, joining the chat layer with the raw agent and
subagent transcripts behind it. The owner's phrase for the goal is "true portable access to
everything the agents are doing without an SSH terminal".

## What it shows

```
 Phone, portrait                               Desk
┌───────────────────────────────┐   ┌──────────────────────────────────────────────────────────┐
│ ◂ #build-lane     Voice│Chan│Work│ │ #build-lane        [ Voice | Channel | Work ]     ⌕  ⚙ │
├───────────────────────────────┤   ├──────────────────────────────────────────────────────────┤
│ 09:00   10:00   11:00   12:00 │   │        09:00       10:00       11:00       12:00   now  │
│ chat ●──●─────●●●──●──────●● │   │ chat   ●──●──────●●●──●────────●●──●──────●   │       │
│  runner-wedged ▬▬▬▬▬▬▬        │   │  runner-wedged   ▬▬▬▬▬▬▬▬▬▬                  │       │
│  cache-key-rewrite    ▬▬▬▬▬▬▬ │   │  cache-key-rewrite           ▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬ │       │
│ coord ███▒▒████▒▒▒████▒▒███  │   │ coordinator ███▒▒█████▒▒▒██████▒▒███████▒▒   │       │
│  ├ worker-a  ████   ███       │   │  ├ fix runner lock   ████████    ████         │       │
│  └ reviewer       ██          │   │  ├ cache key review         ████              │       │
│                               │   │  └ gate run                        ██████████ │       │
│ ┌ 10:42 phase ─────────────┐  │   │ ┌ 10:42–11:12  fix runner lock ─────────────────────┐ │
│ │ Reproduced the wedge;    │  │   │ │ Reproduced the wedge under load; the lock was held │ │
│ │ lock held across await.  │  │   │ │ across an await. Patched, gate green.              │ │
│ │ ⤷ posted 3 messages      │  │   │ │ ⤷ posted to chat 10:58 "runner unwedged ..."  [open]│ │
│ └──────────────────────────┘  │   │ └────────────────────────────────────────────────────┘ │
└───────────────────────────────┘   └──────────────────────────────────────────────────────────┘
```

- **Chat lanes on top**: the channel's main timeline as dots, and each active thread as a bar in its
  thread hue (`#427 server-thread-hues`, so colours match the Threads view), labelled with its slug.
- **Agent lanes below**: the coordinator and its subagents from the transcripts mapped to this
  channel, rendered by the existing wrkviz browser app (phases, lifetime blocks, aggregate bins by
  zoom level, exactly as `ARCHITECTURE.md` specifies).
- **Edges between layers**: a chat message joined to the agent that posted it (`posted_by`, the
  posting join in appendix 2) draws a short connector from the agent's phase to the dot; an owner
  message that an agent received as a prompt draws one the other way.
- **Tapping** a phase opens its summary card and, with the transcript capability, its transcript;
  tapping a chat dot opens the message in the Channel view (the existing jump-to-message).

The top-bar switch (`#view-switch`, a two-position switch today) becomes a three-way segmented
control. On a channel with no mapped transcripts the Work tab shows the chat lanes alone, which is
still a useful hour-by-hour picture with the hourly summaries attached.

## How a channel finds its transcripts

Three ways, in order of preference:

1. **Discovered by the posting join.** Any transcript session whose tool calls posted a message to
   this channel (normalized match, appendix 2) is a candidate. The server lists candidates in
   Settings ("These agent sessions post to this channel: coordinator on build-box-a, started Oct 21.
   Show them in Work?"), and the owner accepts. Subagents follow their coordinator.
2. **Declared**, in configuration, as wrkviz's project config does today:

   ```toml
   [[work.teams]]
   channel = "C0123"                 # the chat channel
   host = "build-box-a"              # a host label, never a hostname
   harness = "claude"                # claude | codex | muse | orc
   root_session = "4f2c…"            # coordinator session; continuations listed explicitly
   ```
3. **From agentctl's registry**, where a session's chat-bridge configuration names its channel. This
   is the same fact as (1), learned before the first post.

## Building it per channel

- **Deterministic, automatic.** On a timer (every minute for ingest, every few minutes for export)
  and on demand when the tab opens, the server polls transcript sources by cursor, updates the
  store, and re-exports schema 3 for the channel. Exports are incremental: adding one day rewrote
  387,547 bytes of the 38 MB measured archive (`ARCHITECTURE.md`). Zero tokens.
- **Paid summaries are not built by opening the tab.** Phase and agent summaries appear where they
  exist; missing ones degrade to the transcript and statistics, as wrkviz builds do now. A
  "Summarize this view" button queues the visible phases as an on-demand batch with its estimated
  cost shown first (appendix 6).
- **Served by vibe-talk** under `/work/<channel>/` with the wrkviz delivery rules ported to Rust:
  gzip sidecars, strong ETags, single `Range` with `If-Range`, no directory listing, refusal of
  snapshot paths. The browser app is the same `app.js`; the page embeds it in the tab.

## Getting transcripts from another host

The coding agents' raw transcripts live on the hosts where the agents run. The chat deployment
that the owner opens may be on a different host.

| Transport | How | Latency | New moving parts | Use when |
|---|---|---|---|---|
| Co-located | Read the files | ms | none | the chat deployment runs on the agents' host |
| SSH pull of appended bytes | For each file, `stat` and read from the cursor's byte offset; verify the prefix digest of the last block; whole-file only on a new file | 1 to 3 s per poll round (estimate) | an SSH key restricted to a read-only command | the first remote deployment; reuses the fetch the owner already runs |
| A small read-only agent | `logbook-agent` on the agents' host runs the transcript `Source`s and answers `poll(cursor)` with normalized batches, over an SSH-forwarded socket or mutual TLS | under 1 s | one daemon per host | several channels watching one host, or live-ish updates |
| Pushed archive | The agents' host builds the schema-3 export itself and syncs it | minutes | a timer on the agents' host | the deployment host may not reach the agents' host at all |

Recommendation: **co-located first, then SSH pull, then the agent if latency matters.** All three
produce the same `Batch<TranscriptMeta>`, so the store and the UI do not care which. The cursor's
prefix digest makes a rewritten or truncated remote file a refusal, as wrkviz's append guard does.

## Security and privacy

Transcripts are more sensitive than chat. Tool payloads hold file contents, absolute paths under the
owner's home directory, command output and whatever credentials leaked into a log line; wrkviz keeps
them gitignored for that reason (`ARCHITECTURE.md`, "Tool payloads are stored"). Serving them to a
phone over the internet is a new exposure, so the default is the narrowest useful one.

| Rule | Detail |
|---|---|
| Three transcript capability levels | `summaries` (phase, agent and rollup summaries, the timeline shape, counts); `events` (prompts, responses, inter-agent messages, tool names and timing); `payloads` (tool input and output). Configured per deployment; **default `summaries`** |
| Write token only | Transcript events and payloads require the write-scope token. The read token (what a voice bridge holds) gets `summaries` at most |
| Per-channel scoping | A channel's Work tab and every API behind it show only the teams mapped to that channel; a request naming another team is refused, not filtered |
| Payload redaction | Payloads served at `payloads` level pass a secret scanner (token shapes, private keys, `Authorization:` headers) and are shown with redactions marked; the original is never sent |
| Nothing to the voice vendor by default | Transcript text never enters a voice call. The research agent (appendix 9) may quote short excerpts in an answer, and the call's settings say so |
| No listing, no snapshots | The static handler refuses directory listings and any `source_snapshots` path, as wrkviz's does |
| Logs | Ids, counts and byte lengths only, as vibe-talk's access log |
| Purge | vibe-talk's purge erases its own index and exports of transcripts; it does **not** delete the agents' original logs or the owner's wrkviz archive, which vibe-talk only reads |
| Remote reads | The SSH key or agent is read-only, restricted to the configured transcript roots, and can only return bytes after a cursor |
| Session hygiene | The Work tab's API responses are `private, no-store`; the phone's offline cache (`#18 offline-message-cache`) does not keep transcript data |

The open question is the default level for a deployment the owner reaches from a phone over the
public internet (question 6 in the main proposal).
