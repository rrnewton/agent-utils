# vibe-talk next: the master plan

2026-10-23, `#455 vibe-talk-next-plan`. Working design for the owner; nothing here is implemented.
This document ties the earlier plan documents into one design on one data model, and maps it to
versions. It keeps the overview. The detail stays where it was written, and each section links
into it. The terms are those of [TERMINOLOGY.md](TERMINOLOGY.md).

| Earlier document | What it now holds for this plan |
|---|---|
| [`2026-10-22-side-agent-helper.md`](../2026-10-22-side-agent-helper.md) | The Ask panel and research's quick depth: briefing shape, UI actions, costs |
| [`2026-10-23-wrkviz-vibe-talk-synthesis/`](../2026-10-23-wrkviz-vibe-talk-synthesis/README.md) | logbook's types, the summary ladder, the wrkviz port, the Work tab, summarization policy, reports, voice briefings and the availability bar, deep research, measurements |
| [`2026-10-23-replication-model.md`](../2026-10-23-replication-model.md) | Replicas, lattices, the send log keyed by idempotency key, derived overlays, changes C1 to C9, the property tests |
| [`2026-10-22-voice-tool-audit.md`](../2026-10-22-voice-tool-audit.md) | What the voice agent was given and how it failed; prompts v11 and v12 |
| [`2026-10-22-channel-bars-layout.md`](../2026-10-22-channel-bars-layout.md), [`2026-10-22-thread-list-animation.md`](../2026-10-22-thread-list-animation.md) | UI proposals on their own track, awaiting answers |

## Answers first

**Terminology.** The storage layer is **logbook**, and its chat half, the **chat store**, is the one
server-side copy of provider messages (it *is* the cache). Fixed-unit, model-written, cached text is
a **summary**, made by **the summarizer** along the **summary ladder**. Open questions go to
**research**, one service with a **quick** depth (one call over a deterministic **briefing**) and a
**deep** depth (a read-only tool loop, run as a research job). The channel view's surface for it is
the **Ask panel**. The **voice agent** consumes summaries and calls research as a tool. The
visualization is the **Work tab**, drawn by **wrkviz**'s timeline app. "Side agent" and "helper" are
retired because they name a screen position and hide the distinction that matters. The full
proposal, with rejected alternatives, is one page: [TERMINOLOGY.md](TERMINOLOGY.md).

**Does one storage layer serve live serving, the wrkviz-style view, and a clean sync model?** Yes,
under three rules, each of which the earlier documents already found separately:

1. **One copy, two read contracts.** The chat store holds each message once. *Serving* a page read
   from it uses the strict serve rule (today's `shadow::decide`): answer only what a live read would
   answer as well. *History* (summaries, research, the Work tab, reports) uses a separate, eventual
   contract: a per-channel watermark, `complete_through`, advanced by the backfill walk. The rows are
   the same; only the predicate differs. Conflating the two contracts is why the cache served
   almost nothing on Slack (synthesis appendix 1).
2. **One version per row, shipped to every consumer.** A row's version is the provider's edit time
   where there is one, else the server's read-start time (`observed_ms`). Each source also has a
   change counter (`change_seq`). The server already merges by these. The plan sends them to the page
   (C7), so the page's copy becomes a true replica of the chat store, and summaries and exports judge
   freshness by the counter, never by clocks. Every memory that answers a read reports its true age
   (`as_of`).
3. **Writes are operations, not state.** vibe-talk writes nothing to messages upstream except posts.
   A post is an operation in a **send log** keyed by its idempotency key, held in the owner store. A
   fast local update, such as a reply that promotes its message into a thread, is a **derived
   overlay**: `provider state ⊔ pending operations`, computed at projection time and never written
   into the stored copy. It appears at once, survives any stale read, and vanishes without trace if
   the operation fails. That is the replication model's §5, and it needs nothing from the store but
   the versions of rule 2.

The detail is in "The one data model" and "Sync and replication" below.

**API over CLI for research.** Settled by the owner on 2026-10-23: research calls the Messages API
from the server, holds no tools beyond read-only logbook queries, and answers in validated
structured output.

**Versions.** vibe-talk is still `0.0.1`. Tag what is deployed today `vibe-talk-v0.1.0`, release this
plan's milestones as minor versions from `0.2.0` (Ask), and reserve `1.0.0` for frozen contracts and
proven correctness, not for feature completeness. See "Versions".

## The architecture in one picture

```
   chat providers: Slack, Discord, a chat bridge                    agent hosts
   (native clients write here; vibe-talk only creates posts)       (Claude, Codex, Muse, Orc logs)
        │ live push and tail      │ reads, relist, backfill   ▲ keyed posts         │ byte cursors
        ▼                         ▼                           │                     ▼
 ┌─ vibe-talk server ─────────────────────────────────────────┼─────────────────────────────────┐
 │  ingest ── Batch<ChatMeta> ──┐                             │   ┌─ owner store ─────────────┐ │
 │                              ▼                             │   │ send log (key → outcome)  │ │
 │  ┌─ logbook ───────────────────────────────────────────┐   └───│ Done, bookmarks, hues,    │ │
 │  │ chat store              transcript store            │       │ pins, settings, read marks│ │
 │  │  rows, tombstones        events, agents, phases     │       │ voice turns, research log │ │
 │  │  coverage, watermarks    cursors (append-only)      │       └───────────────────────────┘ │
 │  │  versions, change_seq                               │                                     │
 │  │ artifacts: summaries + receipts   topics   search   │                                     │
 │  └──┬────────────┬──────────────┬──────────────┬───────┘                                     │
 │     │serve rule  │history       │history       │export (schema 3)                            │
 │     ▼            ▼              ▼              ▼                                             │
 │  pages, SSE   summarizer     briefing ───▶ research: quick (one call) | deep (job, tools)    │
 │               (the ladder)                  Messages API, citations checked against logbook  │
 └──────┬──────────────────────────┬──────────────────────────────┬──────────────┬──────────────┘
        │ HTTP + SSE: versions,    │ MCP: chat tools, summaries,  │ research API │ /work/<ch>/
        │ client_key, ops          │ topics, research             │              │
        ▼                          ▼                              ▼              ▼
  page replica                voice agent                     Ask panel      Work tab
  (channelCanon + outbox)     (briefing at start,             (research      (wrkviz timeline
  Channel view, bar           summaries, research tool)        log, actions)  app)

  wrkviz CLI (Rust) runs logbook on the agents' host: ingest, index, query, summarize, build.
```

Two boundaries matter more than any box:

- **logbook knows what was said and what it means; the applications know who is asking and how to
  show it.** Everything provider-authoritative or derived from it is in logbook. Everything the owner
  does or intends is in the owner store. That is exactly the replication model's split between
  three-party state (§2a) and two-party state (§2b).
- **Deterministic work is automatic; work that spends tokens runs on demand or in batches** (the
  owner's rule, synthesis appendix 6). Ingest, indexing, watermarks, briefings, exports and
  freshness are free and automatic. Summaries are scheduled and budgeted. Research runs only when
  asked.

## Components and their interfaces

| Component | Responsibility | Interface (sketch) | Detail |
|---|---|---|---|
| **Ingest** | Turn every provider observation into a batch: live pushes (`POST /api/v1/live/events`, the Slack tail `POST /api/v1/live/tail`), the poller, reads made for pages and tools, the relist, the backfill walk | `Source<ChatMeta>::poll(cursor, budget) -> Batch`; every batch carries `read_began` and a `Claim` (listing with coverage, prefix, or sighting) | synthesis 02 "Sources"; replication §4.1 |
| **Chat store** (logbook) | Rows, tombstones, coverage, heads, threads, FTS; the merge rules of `cache/merge.rs`; watermarks; change counters | `apply(Batch)`, idempotent with the cursor in the same transaction; `serve(read) -> Served \| Miss(reason)` (the strict rule); `range`, `get`, `search`, `watermark`, `tallies`, `erase`, each taking a `Capability` | synthesis 02 "The store", 04 |
| **Transcript store** (logbook) | Normalized agent events, the agents side table, phases, cursors with prefix digests | the same `Store<M>` trait with `Claim::Prefix`; `groups(source, span)` | synthesis 02, 03 |
| **Summarizer** | The ladder: message one-liner, thread name and sentence, chat hour and day, agent phase and lifetime, rollups, reports; scheduling and the daily ceiling | `Summarizer::stage(unit) -> Staged` (deterministic), `ModelBackend::run(jobs) -> artifacts + receipt`; `freshness(unit) -> Absent \| CaughtUp \| Behind \| Unproven` | synthesis 02 "Summaries", 06 |
| **Briefing builder** | Deterministic, fenced, owner-marked rendering of a scope and span: topics, thread index, availability line, newest messages, full text grouped by thread within a budget; append-only so a provider prompt cache survives | `briefing(scope, span, budget, cap) -> Briefing { fenced, fed_ids, bounds_sentence, token_estimate }`; `append(new batch) -> Fenced` (thread-grouped, edits as correction lines) | side-agent "Context strategy"; synthesis 08 P1 to P5 |
| **Research** | Open questions. Quick: one call over a briefing, warm cache. Deep: a read-only tool loop over logbook, seeded with a briefing, budgeted, as a job | `POST /api/v1/research {question, scope?, depth, origin}` → `{job_id}` at once; SSE `research.step` / `research.done`; `Answer { written, spoken?, citations[], coverage, actions[], answerable }`. Every id is checked against logbook (deep) or the fed ids (quick); deep `coverage` is checked against the job's tool log | side-agent "UI-control protocol"; synthesis 09 |
| **Ask panel** | The UI for research: research log, citation chips, highlight, draft; off by default, lit while active, "catching up" while the briefing warms | consumes the research API and SSE; highlight = jump-to-message with a flash; draft = fill the named composer, never send | side-agent "UI" |
| **Voice agent** | Talking, orientation from a small briefing, delegation | system prompt v13 with a start briefing; MCP tools `recent_activity`, `read_page`, `find_message`, `read_message`, `count_messages`, `post_reply`, plus `summaries`, `topics`, `research`, `research_status`, `research_cancel`; a silent `context` bridge frame (hosted: `contextual_update`) | synthesis 08, 09; voice audit |
| **Serving layer** | Pages, deltas and the SSE stream for the page replica | each copy carries `observed_ms` and `edited`; each newest page carries the stretch it proves (C7); a self-posted stream event carries `client_key` (C2); `POST …/reply {idempotency_key}`, `POST …/reply/outcome` | replication §7 |
| **Page replica** | The page's copy of the chat store and owner state, plus the outbox | `projection(replica, outbox) -> rows + phantoms per scope`; merges copies by the server's versions (after C7) | replication §4.1, §5 |
| **Owner store** | Owner state (unchanged), plus the persisted send log (C5) and the research log | existing `StateStore` methods; `post_keys(key, channel, text_hash, placement, outcome, at_ms)`; `research_log(job, origin, question, answer, citations, coverage, at_ms)` | replication §2b, C5 |
| **Work tab** | A channel's chat lanes and the agents behind it | `GET /work/<channel>/…`: the schema-3 export, served with wrkviz's delivery rules; capability levels summaries / events / payloads | synthesis 05 |
| **wrkviz CLI** (Rust) | The port, over logbook plus the existing archive format | `wrkviz ingest`, `index`, `query`, `summarize`, `build`, `serve`; archives stay readable by the Python edition until it is retired | synthesis 03 |

## The one data model

Every piece of state has exactly one home and one authority.

| Class | Authority | Home | Replicas | Merge |
|---|---|---|---|---|
| Message existence, content, edits, deletions, reactions, thread membership | the provider | chat store | page replica | versioned register per id, epoch tombstones, absence only from a listing over a proven stretch (replication §4.1, §4.2) |
| Agent transcript events | the vendor log file | transcript store | the wrkviz archive; Work tab exports | append-only prefix with a digest; a rewrite is refused (synthesis 02) |
| Sends | the provider holds the truth; the key names the operation | send log in the owner store (in memory today, persisted by C5) | the page's outbox | grow-only map key → produced ids, plus monotone "attempted" and "refused" bits (replication §4.3) |
| Done, bookmarks, hues, pins, settings, read marks | the server, serializing the owner's intents | owner store | page | per class, as today: idempotent sets with a read-your-writes overlay, compare-and-set revisions, first-writer-wins (replication §4.4, §4.5) |
| Summaries, topics, search index, tallies, exports | derived | logbook artifacts and indexes | none needed | content-addressed by exact input; freshness by `change_seq`; never synchronized, only recomputed (replication §4.6) |
| Research log, voice turns | the owner's conversation | owner store | page | append-only |
| Optimistic display (phantom rows, pending counts, a new thread's card and colour) | nobody: derived | none | computed on each page | `provider state ⊔ pending operations ⊔ known replies` at projection time (replication §5) |

### The caches, all of them

The owner asked for caching to be part of the story. There are eight caches, and the plan keeps
each one honest about its authority and its age.

| Cache | Holds | Authority | Version or key | Plan |
|---|---|---|---|---|
| **Chat store** (today's message cache) | provider messages, up to 90 days and 256 MB by default | provider | per-row version; per-source `change_seq`; coverage | becomes logbook's chat store (cutover C3); the one server copy; modes off, shadow, serve |
| **Page replica** (`channelCanon`, `localStorage`) | a trimmed copy of the chat store and owner state | chat store | read-send order today; the server's versions after C7 | stays; merges like the server (C7) |
| **Held recent windows** (`recent.rs`) | about 50 messages a channel, in memory | provider, through reads | read time | stays for the voice agent's deadlines; later reads the chat store when its watermark is fresh |
| **Slack Flat memory**, bridge caches | recent pages, up to 15 minutes | provider | must report `as_of` | rule: every answer from memory reports its true age (replication §4.1) |
| **Send log** (`PostKeys`) | outcomes of keyed posts, 24 h | provider outcome as learned | idempotency key | persisted in the owner store (C5) |
| **Summary artifacts** | model outputs | derived | summarizer id and version, schema, backend, exact payload | logbook artifacts with receipts (cutover C2); never paid for twice |
| **Timeline exports** | schema-3 files per channel and team | derived | byte-idempotent rebuild | Work tab |
| **Provider prompt cache** | the prefix of a research briefing | the model provider | byte-identical prefix; 5 min or 1 h | briefings are append-only; a session restarts with a fresh briefing past about 60 % of context |

## Live serving: the chat store's modes and the ingest work

The cache has three modes, set by `[message_cache] enabled` and `serve`:

| Mode | Config | Fills | Answers reads | Use |
|---|---|---|---|---|
| off | `enabled = false` (shipped default) | no | no | today on the busiest deployment |
| shadow | `enabled = true`, `serve = false` | yes | no; records per consumer what it *would* have served and why not | measure before serving |
| serve | `enabled = true`, `serve = true` | yes | the newest timeline page at once (refreshed in the background) and a fresh newest window, where the serve rule proves them | after the shadow shows a servable rate and no wrong answers |

**What the in-flight ingest lane changes** (`#452 slack-live-ingest` and `#453 cache-backfill`,
implemented on their branch and not yet on `main` at the time of writing). The Slack tail's records
now carry message text into the store directly, rather than prompting a re-read. A periodic
**relist** (off by default) lists each fed channel's newest page and recently active threads as a
listing with coverage, so it can detect edits and deletions. The declared **backfill walk** is
built: `adaptive` by default, a page at a time while the provider is idle, up to 20 pages a channel,
every six hours. On the fake provider the newest-page shadow rate rose from 0 of 10 to 10 of 10, and
that came from the relist. The backfill adds older history, which is what watermarks and summaries
need. None of this is measured on a live deployment yet. Because `adaptive` is the default, a
deployment with a cache open starts backfill reads on upgrade.

**The plan for serving:**

1. Land the ingest lane and enable the cache in **shadow** on every deployment where research or
   summaries will run (owner question D1). The busiest deployment has none today. Its adapter
   serializes reads at about 4 s each, so a briefing built from upstream reads would take minutes.
2. M0: read the shadow's per-consumer miss counts (`message_cache.shadow[].misses`) for a few days
   with the relist on.
3. Turn **serve** on per deployment for the newest timeline page when the shadow shows it servable
   and no wrong answers. Windows on Slack stay unservable by rule (its feed carries no deletions).
   That is correct, and nothing in this plan depends on it.
4. History needs no serve mode: the backfill and watermarks give summaries, briefings and the Work
   tab their "complete through" without touching the serve rule.

## Sync and replication

The topology is two star-shaped links in series, provider ↔ server ↔ pages, and the server is the
only replica that sees both (replication §1). The merge problem for message data is not concurrent
writers but observations of different ages arriving out of order. The server solves that soundly
today. The page does not yet, because it ranks copies by the order it sent its reads, which a stale
memory can invert (F3, race R9, the likeliest cause of the vanished reply).

**Fast local updates.** Every optimistic action is an operation in a local log. It has display
effects, derived at projection time, and durable effects, applied only once the outcome is known.
The worked case is reply-promotes-thread (replication §5.3). The owner replies to X on Main, and at
once X shows "1 reply, sending", the reply appears at the foot of X's thread and the Threads view
gains X's card. All of it is derived from the pending operation and none of it is written. On the
200, the real reply replaces the phantom in the same place. Then X is marked Done and the provider
read cursor moves. On failure the operation stays with its text, marked "Not sent", until the owner
retries, edits or dismisses it.

**Server merges.** The chat store joins every batch with the rules of `cache/merge.rs`: newer version
wins, a push never revives a deletion, absence deletes only inside a listing's proven stretch and
below the unsettled band. The send log joins by key: a repeat of a key gets the original outcome,
never a second post. A restart keeps it once C5 lands.

**The owner's decisions on sends (2026-10-23).**

- **Look upstream before Retry (C6): yes.** After an uncertain single-part send on Slack, the server
  reads the destination before it offers Retry.
- **A failed reply whose message was removed upstream** (race R2) is drawn in the gap in Main where
  the message was, with the outbox foot as the fallback.
- **Edit of a message that may already have posted** mints a new key (the send log stays
  append-only) and warns that the old message may already be in the channel. Edit is never refused.
- **An invisible operation-key marker: yes**, wherever the provider carries one invisibly. vibe-talk
  puts the key on every post through whatever the transport exposes:

  | Provider | Mechanism | What it gives the send log |
  |---|---|---|
  | Slack | message `metadata` (`event_type`, `event_payload`): invisible in the client, returned by the API when metadata is requested | C6 matches on the key, not on the text |
  | Google Chat | a client-assigned message id | upstream idempotency: a repeat of the id cannot post twice, so Retry is safe without a read |
  | Discord | `nonce` with `enforce_nonce` | upstream deduplication of a repeat inside the provider's nonce window; the read covers anything older |

  Where a transport (a bridge, an adapter) does not pass the mechanism through, the server falls back
  to C6's text match and says so in its diagnostics.

**What remains, and where it lands.** C1 to C9 are the replication model's changes. Synthesis
appendix 4's steps are a different list, always written here as "cutover C1" to "cutover C6".

| Change | What | Milestone |
|---|---|---|
| C1 | identify a send by its key, not its placement | done on the `#448 send-dedupe` branch |
| C2 | carry `client_key` on the self-posted stream event | 0.1.x, cl-dedupe lane |
| C3 | resolve unconfirmed keyed sends on page load | 0.1.x, cl-dedupe lane |
| C4 | reply-promotes-thread as a derived overlay, after reproducing F2, with C7's interim receipt fix for R9 | 0.1.x, cl-dedupe lane |
| C5 | persist the send log in the owner store | 0.3.0 Sync |
| C6 | after an uncertain single-part send, look upstream before offering Retry, matching on the operation-key marker where the provider carries one | 0.3.0 Sync (owner: yes) |
| C7 | send the server's versions to the page; retire read-send order | 0.3.0 Sync |
| C8 | the served root counts its own posted reply | 0.3.0, or drop |
| C9 | hue eviction, read-mark epochs, observed-remove Done | 0.3.0, as answered (E7) |
| §8 | model-based property tests on server and page | 0.3.0; required for 1.0 |

## Summaries, briefings and research on the same data

All three read logbook through a `Capability`, and all three render third-party text through the
same fence with the owner's messages marked (`#438`). They differ in who chooses the input and
whether the question is fixed:

| | Summary | Briefing | Research |
|---|---|---|---|
| Input chosen by | the unit (deterministic staging) | scope, span and budget (deterministic) | quick: a briefing; deep: the model, through read-only tools, seeded with a briefing |
| Question | fixed per summarizer | none: it is context | the owner's, verbatim |
| Spends tokens | yes, scheduled and budgeted | no | yes, only when asked |
| Cached as | a content-addressed artifact | provider prompt cache, by prefix | the research log (the answer), not reused as truth |
| Freshness | `change_seq` and watermarks (`CaughtUp`, `Behind`, `Unproven`) | states its bounds in words | states what it read (`coverage`) and up to when |

**Quick versus deep.** The owner's two preferences for open questions do not conflict once depth is
a parameter. The Ask panel's default is quick: push-fed, append-only, every thread in scope named,
full text of a bounded span, one call per question, so no model chooses what to read. Deep is pull
because history is too large to push. It is seeded with the same briefing, budgeted at about 25 tool
calls and a wall-clock cap, and it must say what it searched; the server checks that against the
tool log. Quick escalates to deep only by asking ("This needs older history or the transcripts.
Research it properly?"), never silently.

**One research log.** Every research answer, whether asked in the panel or by voice, is kept in the
owner store's research log and shown in the Ask panel. A voice-started job also shows its steps as a
card in the Voice view, and its short spoken answer is offered at the next pause in the call. That
replaces the synthesis's separate "research card" history with one log, which answers "where was
the answer to X?" for research answers too.

## The voice agent

The audit's lesson holds: most of the voice agent's failures came from what it was handed, not from
the model. Prompts v11 and v12 shipped (full-text catch-up, owner identity, plain speech, honest
abilities). What this plan adds, in order of value:

1. **Delegation (0.5.0).** The `research` tool and a v13 trigger rule: "research …", "dig into …", or
   an offer when `recent_activity` falls short. The job runs on the server, and the call carries on.
2. **A start briefing (0.5.0).** The topic list, the availability line, the newest index and any
   caught-up hour summaries, all read from logbook in milliseconds, about 3.3 k tokens against a
   proposed 4 k budget (synthesis 08). The voice agent can then say "the runner-wedged thread" the way
   the page does.
3. **Summaries by tool and a "fully ready" push (0.5.0).** `summaries` and `topics` tools; a short day
   narrative pushed silently through a new capability-gated `context` frame (owner question F1),
   offered at a pause.
4. **Measurement (M0, then every prompt or model change).** The voice eval harness of synthesis
   appendix 9. Its tasks 2 (usable context) and 8 (research trigger precision and recall) come first,
   because the briefing budget and the hand-off design rest on them.

Outside this repository, and unchanged: the private stack's speech synthesizer runaway guard and
per-sentence token ceiling (voice audit, "Proposed mitigation"), for that stack's owners.

## wrkviz and the Work tab

The port is the same logbook with a second metadata type (`TranscriptMeta`). Its contract is the
archive format, not the code: Rust reads every archive the Python wrote, and stages byte-identical
payloads so no cached summary is paid for twice, and builds byte-identical website data (synthesis
03). It runs on its own track (W1 to W3 below), because only the Work tab depends on it. vibe-talk
users feel the port only through the Work tab and through deep research reaching transcripts.

The Work tab (synthesis 05) is a consumer of logbook's export: chat lanes in thread hues on top, the
agents mapped to the channel below, joined where a posting join matches. It defaults to the
`summaries` capability, needs the write token for events, and redacts payloads. With chat lanes only,
it works for any channel as soon as hourly summaries exist, before any transcript is ingested.

## Versions

**Today.** `vibe-talk/Cargo.toml` says `0.0.1` and has never been bumped, although vibe-talk has
been deployed continuously on two providers. The repository's plain `vX.Y.Z` tags belong to another
component's history, so vibe-talk needs namespaced tags.

**Proposed scheme:**

- **Tags** `vibe-talk-vX.Y.Z` on `main`, with `vibe-talk/Cargo.toml` bumped in the same commit
  (`vibe-talk: release 0.2.0`). The health route and `vibe-talk --version` already report the Cargo
  version, so once it is bumped a deployment says what it runs.
- **`0.1.0`: today's deployed state**, tagged retroactively on the commit currently deployed (the
  coordinator knows which; if the two deployments differ, the older one). No code change but the
  Cargo bump on `main`.
- **`0.1.x`: patch releases** for work inside the current architecture, which is what the in-flight
  lanes are: the send dedupe and reply-promotes-thread, the Slack tail ingest and backfill, UI polish.
  A deploy of new code bumps the patch.
- **`0.2.0` onward: one minor per plan milestone** below, in release order. Pre-1.0, a minor may
  change configuration or the page's storage format, with a migration and a note in the README's
  upgrade section.
- **logbook** and **wrkviz (Rust)** carry their own crate versions. logbook stays `0.x` and
  unpublished. The Rust wrkviz reaches `1.0` when the Python edition is retired, as agentctl did.

**What justifies `1.0.0`:** a promise about what exists, not a feature count.

1. **Frozen contracts** with a written deprecation policy: the configuration file, HTTP API v1, the
   MCP tool manifest, and the `vibe-talk-v1` bridge protocol (with the `context` frame).
2. **Data safety.** Every store migration is tested as an upgrade from every release since `0.2.0`.
   Purge, retention and access-loss erasure are tested end to end, and the `archive` class is
   decided.
3. **Sync proven.** The replication properties of §8 run in the gate on server and page. Sends are
   exactly-once across restarts (C5). No vanished or doubled reply has been seen in four weeks of
   use.
4. **Serve on by default** for the reads the shadow proves, on at least two providers, for four weeks
   with no stale answer.
5. **Research and voice measured.** The eval harness has thresholds the owner sets (for example
   research trigger precision and recall, honesty tasks, and every citation validated by
   construction), and the spend ceiling is enforced and visible.
6. **Operable.** Deploy, rollback, upgrade and diagnostics (serve rate, spend, freshness) are in the
   `userguide`.

The Work tab, transcripts and reports are not required for 1.0. They are additive, and can arrive
as 1.x minors.

## Milestones

Sizes, as in the synthesis: S is days, M one to two weeks, L several weeks, of one agent's focused
work with review. Every milestone is landable and deployable alone.

| Version | Milestone | Size | Depends on | Delivers | Folds in |
|---|---|---|---|---|---|
| — | **M0 Measure and decide** | S | — | the owner's answers to the blocking questions (marked ★ below); `count_tokens` on a real day's briefing; the shadow's miss counts per consumer with the relist on; the voice eval harness, tasks 2 and 8; the posting-join rate on a real week; Haiku 5.5 on wrkviz's model-comparison plan | synthesis M0 |
| `0.1.0` | **Baseline** | tag | — | a name for what is deployed | — |
| `0.1.x` | **In-flight lanes** | — | — | send dedupe (C1 to C3), reply-promotes-thread (C4), the Slack tail ingest, relist and backfill, UI polish | replication C1 to C4 |
| `0.2.0` | **Ask**: quick research over chat, the Ask panel | M | 0.1.x ingest and backfill; the cache on (shadow) where it runs; M0 (scope, span, model) | the briefing builder; `[research]` config and API; structured answers with validated citations; highlight and draft; off by default, lit while active, push-fed; the research log | side-agent P1, P2 |
| `0.3.0` | **Sync**: one copy, correctly replicated | M | 0.1.x (C1 to C4) | the send log persisted (C5); the server's versions to the page (C7); C6 with the operation-key marker per provider; C8, C9 as answered; the property tests; serve on for the newest page where M0's shadow shows it safe | replication C5 to C9, §8 |
| `0.4.0` | **logbook and the chat ladder** | L | 0.3.0 (the version contract is final before the module moves); M0 (crate location, models, budget) | the crate; `Fenced` and the Messages API backend moved; summaries as artifacts with receipts; the cache becomes the chat store; watermarks and freshness; `chat-hour` and `chat-day` on demand; thread and term topics; the availability bar; spend line and daily ceiling | synthesis M1, most of M2; cutover C1 to C5 |
| `0.5.0` | **Voice, briefed and delegating; deep research on chat** | M to L | 0.2.0, 0.4.0 | the start briefing; `summaries`, `topics`; the `context` frame; `research`, `research_status`, `research_cancel`; deep research jobs over chat with steps, cancel and checked coverage; prompt v13; the eval harness as an opt-in suite with a capability card per voice model | synthesis M2 (voice part), M3; side-agent P3 |
| W1 | **Transcripts in Rust** (wrkviz track) | L | 0.4.0 (the crate exists) | `TranscriptMeta`; Claude and Codex sources with byte cursors, subagents included; groups and phases; the posting join; `wrkviz index`, `query`, `serve` over old archives | synthesis M4 |
| W2 | **Schema-3 export in Rust** | M | W1 | the export, differential-tested against Python; the delivery handler as a reusable Rust module | synthesis M5 (first half) |
| `0.6.0` | **Work tab** | L | W2 | the third view; channel-to-team mapping by join, configuration and the agentctl registry; capability levels and redaction; "Summarize this view" with an estimate | synthesis M5 (second half) |
| `0.7.0` | **Research reaches transcripts; batches; reports** | M to L | 0.5.0, W1, 0.6.0 | research tools over transcripts within the capability; nightly batches for phases and agent names within the budget; daily technical rollups; weekly reports drafted and reviewed in the app | synthesis M6, M7 |
| W3 | **wrkviz parity** | L | W2 | SSH pull and the read-only agent; Orc and Claude-history sources; Muse; losslessness for Claude and Orc; workstream topics; the Python edition retired | synthesis M8; side-agent P4 (older history) moves here as transcripts plus archive |
| `1.0.0` | **Contracts and proof** | M | 0.5.0 at least | the criteria in "Versions" | — |

**Why this order.** Ask comes first because it is the most-requested value and needs no new storage:
thread names, the Messages API client and the fence exist, and the ingest lane fills the cache. It
reads through a narrow read interface that the chat store implements now and logbook implements
after the move. Sync comes before the logbook cutover so that the module moves once, with its
version contract final. Voice comes after both, because a briefed voice agent needs the ladder and
topics (0.4.0), and delegation needs research (0.2.0). The wrkviz track starts once the crate
exists, and it can run in parallel with 0.5.0 on another agent.

**Dependencies:**

```
 M0, measure and decide: feeds every milestone that waits on a ★ question

 0.1.x lanes ─┬─────────────────▶ 0.2.0 Ask ───────────────────┐
              │                                                ▼
              └─▶ 0.3.0 Sync ─▶ 0.4.0 logbook + ladder ─────▶ 0.5.0 voice + deep ─▶ 1.0.0
                                  │                            │
                                  ▼                            ▼
                                  W1 ─▶ W2 ─▶ 0.6.0 Work tab ▶ 0.7.0 transcripts, batches, reports
                                         │
                                         └─▶ W3 wrkviz parity
```

0.2.0 and 0.3.0 can run in parallel on two agents. So can 0.5.0 and W1.

## How the in-flight lanes fit

| Lane | Work | State on 2026-10-23 | Fits as |
|---|---|---|---|
| **cl-dedupe** | `#448 send-dedupe`, then reply-promotes-thread | the branch at `09175ed1` posts each keyed message once whatever the provider (C1 done) | 0.1.x. Next C2 and C3, then reproduce F2 in the page tests, then C4. Its in-memory `PostKeys` is the send log that 0.3.0 persists. |
| **cl-ingest** | `#452 slack-live-ingest`, `#453 cache-backfill` | implemented and closed on its branch (`21c40a29`), not yet on `main` | 0.1.x. It is the prerequisite for 0.2.0's briefings and 0.4.0's watermarks. Review against the replication model's alignment notes: the relist offered as a listing with coverage, every answer from memory with its true `as_of`, stable event ids. Its upgrade note: `adaptive` backfill starts reads on any deployment with a cache open. |
| **mu-polish** | `#450 pack-triangle-hue`, `#451 read-strip-followups` | on its branch | 0.1.x, UI track. The Ask icon joins the floating row at the top of the list, and the availability bar copies the read strip's floating pattern, so both wait for the read-strip follow-ups to settle. |
| UI proposals | channel-bars-layout, thread-list-animation | awaiting answers (group I below) | 0.1.x or 0.2.0, UI track. thread-list-animation's arrival motion is also what a thread promoted by C4 should use when it joins the Threads view. |

## Risks

| Risk | Mitigation |
|---|---|
| The program is large | Every milestone is useful alone. 0.2.0 to 0.5.0 deliver the felt value without the port. |
| Moving the cache breaks its race and privacy guarantees | Move it once, after 0.3.0, with its tests unchanged. Import `#208`'s cache-fill equivalence harness into the tree and rerun it at each cutover step (synthesis 04). |
| C7 touches the page's central merge (`voice.js` is about 52,000 lines) | The property tests (§8) first. Ship C7's interim receipt fix for R9 with C4 in the cl-dedupe lane, so C7 itself is off the critical path for the vanished reply. |
| Serve answers a stale page | Serve stays per consumer and per deployment, after a shadow period with zero wrong answers. Every memory reports `as_of`. Windows on Slack are never served. |
| Briefings are larger than measured, or the voice model's context is smaller | `count_tokens` and eval task 2 in M0. Bounds are stated in words to the model and the owner. Research is the escape valve. |
| Research gives a confident partial answer | Quick is push-fed with every thread named. Deep must state its coverage, checked by the server. Citations are validated, and an invalid one is dropped and said. |
| Token spend creeps | A daily ceiling, split into automatic, batch and on-demand shares. Estimates before batches. Receipts for chat too. A spend line in Settings and the bar. |
| Transcripts leak through a phone | Summaries-only by default, the write token for more, payload redaction, per-channel scoping, `no-store`. Voice-started jobs return only a short spoken answer. |
| The in-flight lanes conflict with the cutover | Land cl-ingest and cl-dedupe before 0.4.0 starts. The cutover begins from `main`, not from a branch. |
| Terminology drifts back | Config, API, tool and module names follow TERMINOLOGY.md from the first commit (`[research]`, `/api/v1/research`, `research` tool, `logbook::chat`). No `helper` name ships. |
| Version tags collide with the repository's other tags | Namespaced `vibe-talk-v` tags. |

## Every open owner question, consolidated

Deduplicated from all the documents above, and grouped. ★ marks a question that blocks the next
milestone. Sources: SA = side-agent, SY = synthesis README, RM = replication model, CB =
channel-bars, TA = thread-list-animation, 03 = synthesis appendix 3, new = this plan.

### Already answered (recorded so nobody asks again)

- **API or SDK, not a coding-agent CLI, for the research backend.** Owner, 2026-10-23.
- **Quick research is push-fed, not tool-pull; two UI powers only (highlight, draft); off by default;
  a lit icon while active; a bulk catch-up on reactivation; grouped by thread in the page's own
  terms; never replies to fed messages.** Owner, in the side-agent request (SA "What the owner asked
  for").
- **The voice agent may hand questions to research**, on a phrase like "research for me X" (SA Q7).
  Owner, 2026-10-23 (synthesis appendix 9). What remains open is the detail in question C7 below.
- **Deterministic work is automatic; token-spending work runs on demand or in batches.** Owner
  (synthesis appendix 6).
- **The store is generic over a precise metadata type for chat and for transcripts.** Owner
  (synthesis appendix 2).
- **The speech synthesizer's runaway** belongs to the private voice stack, not this repository
  (voice audit).
- **Sends (E1 to E4).** Owner, 2026-10-23: look upstream before Retry; a failed reply to a removed
  message goes in the gap in Main, with the outbox foot as fallback; Edit mints a new key and warns,
  and is never refused; an invisible operation-key marker wherever the provider carries one. See
  "Sync and replication".

### A. Names and versions

1. ★ Adopt the terminology in TERMINOLOGY.md? (new)
2. ★ The version scheme: `vibe-talk-v0.1.0` tagged on the deployed commit, one minor per milestone
   from 0.2.0, and 1.0 meaning frozen contracts and proven sync rather than every feature? (new)
3. ★ (before 0.4.0) Is `logbook` the crate's name, and does it live in `rs/logbook` with vibe-talk's
   container built from the repository root, or inside `vibe-talk/`? (SY Q2)

### B. Order

1. ★ This order: Ask (0.2.0), Sync (0.3.0), logbook and the ladder (0.4.0), voice (0.5.0), then the
   Work tab and the port? The synthesis put the ladder and voice first. This plan moves Ask ahead
   because it needs no new storage. (SY Q1, revised)

### C. Research and the Ask panel

1. ★ **Scope.** Every channel on the server, or the one or two on screen? (SA Q1)
2. ★ **Span.** Full text of the last 24 hours plus every thread's name and sentence for 7 days, or
   "since I last read each channel", or chosen when opened? (SA Q2)
3. ★ **Models.** Quick research on Haiku 5.5 (a few cents a day) or Sonnet 5.5 (a dollar or two on a
   heavy day)? Sonnet 5.5 for deep research and reports, and Haiku 5.5 for hourly rollups and topic
   suggestions? Should wrkviz phase summaries move from the coding CLIs to the API? (SA Q3, SY Q3)
4. **Active lifetime.** Switch off after an idle hour, or stay lit until tapped? Follow you across
   devices and reloads? (SA Q4)
5. **Drafts.** Into the named channel's or thread's composer, never sent, with a banner? Ever to a
   channel other than the one on screen? (SA Q5)
6. **The research log.** Keep it and make it searchable like voice transcripts, or discard it when
   research is switched off? (SA Q6)
7. **Voice hand-off detail.** Is "research …" plus an offer when `recent_activity` falls short the
   right trigger? Should a result be read out unasked when it arrives mid-call? May a voice-started
   job read transcript events, not just summaries, given that only the short spoken answer reaches
   the call? (SY Q10)

### D. Storage, history and budget

1. ★ **The cache on.** May the cache be enabled in shadow on every deployment where research or
   summaries will run, and serve be turned on per deployment once the shadow proves it? (SA Q8,
   extended)
2. **History.** May chosen channels keep their messages beyond the cache's 90 days (the `archive`
   class), and is an approved weekly report kept when the messages it summarizes are erased? (SY Q4)
3. ★ (before 0.4.0) **Budget.** What daily token ceiling? Should the spend show in the Ask panel and
   the availability bar? Should nightly batches summarize every transcript phase (about $1 a day on
   Haiku 5.5, $20 on Sonnet 5.5, estimated) or only phases someone will look at? (SA Q3 ceiling, SY
   Q5)

### E. Sync and replication

1. Answered 2026-10-23: yes, look upstream before offering Retry (C6). (RM Q1)
2. Answered 2026-10-23: the gap in Main, with the outbox foot as fallback. (RM Q2)
3. Answered 2026-10-23: a new key and a warning; never refuse Edit. (RM Q3)
4. **An invisible id in provider metadata.** Answered 2026-10-23 for vibe-talk's own posts: yes,
   wherever the provider carries it invisibly. Still open: should the agents' posting tools stamp the
   same kind of id, so the posting join is exact rather than a normalized text match? It is the same
   mechanism. (RM Q4, synthesis 02 "Joining the two layers")
5. Should read-affecting per-device settings ("own messages count as read", and perhaps pack and
   fold state) sync through the server? (RM Q5)
6. When a thread's root is deleted upstream but replies remain, keep the thread with a "message
   removed" root, or drop it? (RM Q6)
7. Does Undo on one device need to leave a Done made on another device (an observed-remove set), or
   is last-action-wins fine? (RM Q7)

### F. The voice agent

1. **Fully ready.** Push a short day narrative silently and offer it, or only say that summaries are
   ready? May the bridge protocol gain a `context` frame? (SY Q8)
2. ★ (for M0) **The eval harness.** May it run against the deployed voice stack (it costs voice
   minutes), or only against a test bridge? (SY Q13)

### G. Transcripts, the Work tab and wrkviz

1. **Exposure.** From a phone over the public internet, should the Work tab default to summaries
   only, or show prompts and responses with the write token? Should tool payloads ever be served?
   (SY Q6)
2. **Remote transcripts.** SSH pull of appended bytes, a small read-only agent on the agents' host,
   or build the export there and sync it? (SY Q7)
3. **Muse.** Where does Muse keep its session transcripts? (SY Q12)
4. **Labels.** Was label or label-suggestion work started outside this repository? Are workstream
   topics the labels you had in mind? (SY Q9)
5. **The archive-local `./timeline`.** Keep generating the dependency-free Python reader in every
   archive after the port? (03)

### H. Reports

1. Which channels and teams make up the product update, on what day and at what time, for which
   audience (plain, technical or both)? (SY Q11)

### I. UI proposals (independent of the data model)

- **channel-bars-layout:** option A, B (recommended) or C (CB Q1); the read filter as a chip (Q2);
  whether the read panel remembers being open (Q3); whether two taps to listen are deliberate enough
  (Q4); audio source in the panel or in Settings (Q5); the keyboard-up control (Q6); whether to fold
  the panel when reading stops (Q7); whether the Talk tile joins the rework (Q8).
- **thread-list-animation:** the gap's timing and fade (TA Q1); which side closes the gap (Q2);
  arrival from far above, and an "arrived below" pointer (Q3); following the highlighted card, and
  holding a thread still while you type an inline reply (Q4); reduced-motion highlight (Q5); scope,
  and a separate motion switch (Q6); how visible "behind" should be (Q7).
