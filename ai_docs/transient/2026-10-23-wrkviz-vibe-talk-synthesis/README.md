# wrkviz + vibe-talk: one store for what the agents say and do

2026-10-23. Design proposal for `#449 wrkviz-vibe-talk-synthesis`; nothing here is implemented.
A working note for the owner to decide on. The questions are at the end; the appendices hold the
inventory, the Rust type sketches and the measurements.

## Answers first

**Where is the side-agent proposal?** `ai_docs/transient/2026-10-22-side-agent-helper.md` (on
`main` since `4fd6aed3`). It proposes a push-fed text helper behind an icon at the top of the page:
server-built context of every thread plus the last 24 hours in full, Messages API with prompt
caching, structured answers with validated citations, a highlight action and a draft action, off by
default. Phase 3 there lets the voice agent ask it; appendix 9 here extends that into the research
agent you described.

**What is the chat digest API, and why did it fail?** `digest_channel` (`GET
/api/v1/channels/{id}/digest`, `vibe-talk/src/ops.rs:1536`, `src/summary.rs`) is **not a
summarizer**. It reads a channel's recent window and returns one line per message: the id, the
local time, the author, and the first 160 characters of the speakable text with code and links
replaced by placeholders. No model, no cost. It failed for you because its chat read had no deadline
and no held copy to fall back on, so behind a slow adapter it outlived the voice runtime's 15-second
tool limit. Since `#437 voice-full-text-catchup` it is no longer offered to the voice agent;
`recent_activity` answers the same question with full text, from the held recent window, within 2.5 s
(8 s cold). vibe-talk's real server-side summarization is two things: a one-sentence summary of a
long message, and each thread's name (a slug such as `runner-wedged`, chosen once) and sentence, both
made by a plain Messages API call to Haiku (`src/summarize/`).

**Labels.** Neither feature exists in this repository. wrkviz has no label or classification code;
the closest pieces are a retired glossary summarizer and an empty "supported semantic concepts"
input that every phase and rollup prompt reserves for exactly this. vibe-talk has no label
suggestions; its thread slugs are the nearest thing. If either was started elsewhere, it has not
landed here (question 9). Appendix 2 designs both as one topics-and-labels feature.

**One correction about wrkviz.** It is not a database and it does not summarize each message. It is
a directory archive (JSON, seekable gzip JSONL, content-addressed artifacts) with summaries per
30-minute agent phase, per agent, and per hour, day, week, month and quarter. Per-message summaries
are vibe-talk's.

## The picture

```
         chat providers                              agent hosts
   (Slack, Discord, a chat bridge)        (Claude, Codex, Muse, Orc transcripts, subagents)
                │ reads, live feed, backfill                 │ cursors: local, SSH pull, or agent
                ▼                                            ▼
        ┌─────────────────────────────── logbook (Rust) ───────────────────────────────┐
        │  Store<ChatMeta>             Store<TranscriptMeta>        topics and labels  │
        │  watermarks, freshness       groups (agents)              search (FTS5)       │
        │  summaries: message ─ thread ─ phase ─ agent ─ hour ─ day ─ week ─ report    │
        │  artifacts + receipts (wrkviz's cache identity)        timeline export (v3)  │
        └───────────────┬───────────────────────────┬──────────────────────┬──────────┘
                        │                           │                      │
                vibe-talk server              wrkviz CLI (Rust)      weekly reports
         Voice │ Channel │ Work tab           archives, query, serve   draft → owner review
         voice agent ──research──▶ research agent (server-side, tools over both layers)
         availability bar, side-agent helper
```

**One storage component** owns items, cursors, summaries, freshness, topics, search and export, for
both families. It is generic over a precise metadata type: `ChatMeta` (thread, author, scope,
reactions, which agent posted it) and `TranscriptMeta` (harness, agent, turn, event kind, tool call,
origin in the vendor file) look nothing alike, and the type system keeps them apart while the store,
the summarizer ladder and search are shared (appendix 2).

## The proposal in ten parts

1. **Inventory** (appendix 1). wrkviz has strict ingestion with append guards, a disciplined paid
   artifact cache with receipts, five summary levels and a fast static timeline; it lacks a server,
   chat, Muse, remote transport and labels. vibe-talk has the chat side, the Messages API client, the
   privacy and scope rules, thread slugs, and a message cache that serves almost nothing on Slack.
   That last one is by design: its serve rule requires a feed that carries deletions or a listing
   from the last minute, and Slack's feed carries additions only, so instant serving and complete
   history are different requirements and should be built separately.
2. **The shared crate, `logbook`** (appendix 2). `Store<M: Metadata>`, `Source<M>` with byte or
   provider cursors, a `Claim`/`Watermark` model that keeps chat's strict serve rule and adds an
   eventual "complete through" for summaries, a `Unit` ladder from one message to a weekly report,
   `Summarizer` and `ModelBackend` traits with wrkviz's content-addressed artifacts, a precise
   `Freshness` (absent, caught up, behind, unproven), topics and labels, `Capability`-guarded
   access, and the exact chat-to-transcript join (an agent's posted message appears verbatim in its
   own tool call). SQLite for the working store; wrkviz's archive files stay the durable record.
3. **The wrkviz port** (appendix 3). A parity checklist row by row, with archive compatibility as
   the contract, a payload-identity test so not one cached summary is paid for twice, a
   byte-for-byte build differential, and a migration that reads old archives in place and indexes
   them into SQLite. Drop: schema 1 and 2 writers, the glossary summarizer, eventually whole-file
   snapshots of append-only logs.
4. **vibe-talk on it** (appendix 4). The owner's state stays in vibe-talk's store; summaries become
   artifacts; the message cache becomes the chat store with its coverage and serve rules moved
   unchanged. A new opt-in `archive` retention class keeps history for reports without weakening the
   disposable `mirror` default. Six cutover steps, each reversible.
5. **The Work tab** (appendix 5). The wrkviz timeline per channel, with chat lanes on top in thread
   colours and the agents below, joined by exact edges; served by vibe-talk with wrkviz's delivery
   rules; transcripts local, pulled over SSH by byte range, or from a small read-only agent.
   Transcript access has three levels (summaries, events, payloads), defaults to summaries, needs the
   write token above that, and redacts payloads.
6. **Summarization policy** (appendix 6). Everything deterministic runs automatically; hourly chat
   rollups run on demand for the window you open; transcript phases, agent names and daily rollups
   run in nightly batches within a budget; reports and topic suggestions weekly. Chat costs cents a
   day on any model. Full transcript coverage costs about $1 a day on Haiku 5.5 or about $20 on
   Sonnet 5.5 (estimates from measured volumes), so phases are summarized when someone will look.
7. **Weekly reports** (appendix 7). Gathered deterministically (summaries, landed commits, closed
   issues, your prompts), merged across channels and machines by one model call with citations,
   checked against the facts, and reviewed by you in the app. Never posted automatically.
8. **Voice context** (appendix 8). At call start, push what makes the first answer right and costs
   milliseconds from the store: the availability line, the topic slugs, the newest messages and any
   caught-up hour summaries. At "fully ready" (estimated 30 to 50 s cold, about 5 s warm), push a
   short day narrative silently and leave hourly detail to a `summaries` tool. The bridge protocol
   needs a new capability-gated `context` frame for a silent mid-call push.
9. **The availability bar** (appendix 8). "in the last [1] day the last [5] hours are summarized,
   containing [43] messages", live, with green borders when the whole window is caught up, amber
   when an hour fell behind, and an expandable hour strip per channel with spend.
10. **The research agent** (appendix 9). "Research for me X" makes the voice agent call `research`,
    which returns in under a second; a server-side agent with read-only tools over chat, transcripts
    and search digs for 15 seconds to 3 minutes, shows its steps on the page, and comes back as a
    short spoken answer plus a cited written one. Same core as the side-agent helper, in a tool-loop
    mode. A harness built on `smoke-agent.py` measures what the voice model can actually do.

## Phased plan

Sizes: S is days, M one to two weeks, L several weeks, of one agent's focused work, with review.
Each milestone is landable alone.

| # | Milestone | Size | Depends on | Delivers |
|---|---|---|---|---|
| M0 | **Measure and decide**: the owner's answers below; the voice eval harness, tasks 2 and 8; `count_tokens` on a real chat day; the cache's per-consumer miss counts; Haiku 5.5 on wrkviz's model-comparison plan | S | — | The numbers the defaults rest on |
| M1 | **`logbook` core**: ids, spans, buckets, `Item<M>`, `ChatMeta`, `Fenced`, `MessagesApi` backend moved out of vibe-talk; artifacts and receipts; vibe-talk's summaries become artifacts (cutover C1, C2) | M | M0 decisions 1 to 3 | Shared types and model client, receipts for chat |
| M2 | **Chat history and the bar**: the cache becomes `ChatStore` (C3); backfill walk and watermarks (C4); tallies, freshness; `chat-hour` and `chat-day` rollups on demand (C5); thread and term topics; the availability bar; voice start push (P1 to P4) and the `summaries` and `topics` tools | M to L | M1 | **The first thing you would feel**: a voice call that knows the vocabulary and says how much it knows |
| M3 | **Research agent on chat**: assistant core, research mode with chat tools, `research` / `research_status` / `research_cancel`, the page card, silent result push (`context` frame on the bridge, `contextual_update` hosted); the side-agent helper's P1 on the same core | M | M2 | "Research for me X" over chat history |
| M4 | **Transcripts in Rust**: `TranscriptMeta`, Claude and Codex sources with byte cursors (subagents included), groups, phases, the exact join; wrkviz `index`, `query`, `serve` in Rust over old archives | L | M1 | Both layers in one store on the agents' host |
| M5 | **Work tab**: schema-3 export in Rust (differential-tested), the static delivery handler in Rust, the third view, per-channel mapping by join and configuration, capability levels and redaction | L | M4 | The timeline on the phone |
| M6 | **Research reaches transcripts**; nightly batches for phases and names within a budget; daily technical rollups | M | M3, M4 | Deep answers about what the agents did |
| M7 | **Weekly reports** across channels and hosts | M | M2, M6 | A reviewed product-update draft every Friday |
| M8 | **Remote and full parity**: SSH pull and the optional agent; Orc and Claude-history sources; Muse; losslessness tables for Claude and Orc; workstream topic suggestions; retire the Python edition of wrkviz | L | M5 | Feature-complete port; one codebase |

**Build first: M2** (after a short M0 and the M1 it needs). It uses only chat data and pieces that
already exist (thread names, the Messages API client, the held windows, the cache file), it fixes the
experience you described (a voice agent that starts immediately, says what is ready, and uses your
terms), and it creates the store, watermarks and freshness that everything later needs. M3 is next
because it turns the voice agent's weakness into a hand-off instead of a wrong answer.

## Risks

| Risk | Mitigation |
|---|---|
| The port re-pays for summaries because staged payloads differ by a byte | Payload-identity tests per summarizer before any Rust `summarize` runs (appendix 3) |
| Moving the message cache breaks its hard-won race and privacy guarantees | Move the module unchanged with its tests; rerun the M5 equivalence harness at each cutover step |
| History retention changes vibe-talk's privacy stance | `archive` is opt-in per channel; the purge stays total; summaries follow their sources (appendix 4) |
| Serving transcripts to a phone leaks secrets | Summaries-only default, write token for more, payload redaction, per-channel scoping, `no-store` (appendix 5) |
| The voice model is weaker than assumed, or its context smaller | Measure first (M0); keep the push small; delegate to research |
| The bridge cannot take a silent context frame | Capability-gated, with the existing `notice` turn as the fallback |
| Token spend creeps | Daily ceiling, estimates before batches, receipts and a visible spend line (appendix 6) |
| Remote transcript transport is fragile or slow | Co-located first; byte-range pulls with prefix digests; refusal, not silent gaps |
| Two editions of wrkviz drift during the port | Differential tests in the gate; retire Python per family, not all at once |
| Scope: this is a large program | Every milestone is useful alone; M2 and M3 deliver most of the felt value without the port |

## Questions for the owner

1. **Order.** Is M2 (chat summaries, the bar, the voice start push) then M3 (research over chat) the
   right first value, ahead of the Work tab and the port?
2. **Where the crate lives.** `rs/logbook` in the Rust workspace, with vibe-talk's container built from
   the repository root, or inside `vibe-talk/`? (The name `logbook` is a placeholder too.)
3. **Default models.** Haiku 5.5 for hourly chat rollups and topic suggestions, Sonnet 5.5 for
   research and reports? Should phase summaries move from the Codex CLI to the API?
4. **History.** May chosen channels keep their messages beyond the cache's 90 days (the `archive`
   class), and should an approved weekly report be kept when the messages it summarizes are erased?
5. **Token budget.** What daily ceiling, and should nightly batches summarize all transcript phases
   (about $1 a day on Haiku 5.5, $20 on Sonnet 5.5, estimated) or only those someone will look at?
6. **Transcript exposure.** For a deployment reached from a phone over the public internet, should the
   Work tab default to summaries only, or show prompts and responses with the write token? Should tool
   payloads ever be served?
7. **Remote transcripts.** For a chat deployment on a different host from the agents: SSH pull of
   appended bytes, a small read-only agent on the agents' host, or build the export there and sync
   it?
8. **Voice pushes.** At "fully ready", should the voice agent receive a short day narrative silently
   and offer it, or only learn that summaries are ready? May the bridge protocol gain a `context`
   frame for that?
9. **Labels.** Was the labels or label-suggestions work started anywhere outside this repository? Are
   workstream topics (suggested weekly, accepted by you) the labels you had in mind?
10. **Research hand-off.** Is "research …" plus an offer when `recent_activity` falls short the right
    trigger? Should research results ever be read out unasked when they arrive mid-call?
11. **Reports.** Which channels and teams make up the product update, what day and time, and which
    audience (plain, technical or both)?
12. **Muse.** Where does Muse keep its session transcripts, so they can be ingested?
13. **Running the voice eval.** May the eval harness run against the deployed voice stack (it costs
    voice minutes), or only against a test bridge?

## Appendices

| File | Contents |
|---|---|
| [01-inventory.md](01-inventory.md) | What wrkviz and vibe-talk have, with code references; the cache-coverage analysis; overlaps and gaps |
| [02-shared-crate.md](02-shared-crate.md) | `logbook`: traits and types in Rust, freshness, topics, the exact join, storage choice |
| [03-wrkviz-port.md](03-wrkviz-port.md) | Parity checklist, migration path, what to drop, tests |
| [04-vibe-talk-on-logbook.md](04-vibe-talk-on-logbook.md) | What moves, retention classes, guarantees, phased cutover |
| [05-work-tab.md](05-work-tab.md) | The third tab, mapping channels to transcripts, transport, security and privacy |
| [06-summarization-policy.md](06-summarization-policy.md) | Automatic versus on-demand versus batch; costs on real volumes; controls |
| [07-weekly-reports.md](07-weekly-reports.md) | Inputs, configuration, pipeline, format, review |
| [08-voice-context-and-bar.md](08-voice-context-and-bar.md) | Push and pull at start and at fully ready, latencies, the availability bar mock |
| [09-research-agent.md](09-research-agent.md) | Tools, hand-off, relation to the helper, model comparison, measuring the voice model |
| [10-measurements.md](10-measurements.md) | Every measured number, with its script and output |
