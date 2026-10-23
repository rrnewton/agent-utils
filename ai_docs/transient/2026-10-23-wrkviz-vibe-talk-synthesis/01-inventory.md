# Appendix 1: Inventory of wrkviz and vibe-talk

Part of `#449 wrkviz-vibe-talk-synthesis`. Read from the tree at `c5b149d5` (2026-10-23). Paths are
relative to the repository root. "Measured" numbers say where they came from; everything else is
read from the code.

## wrkviz (`py/wrkviz/`, Python, about 52,000 lines plus an 8,458-line browser app)

wrkviz turns coding-agent transcripts into a durable, version-controllable archive with paid
summaries and a static Perfetto-style timeline website. It is the transcript half of the owner's
picture. Its contract document is `py/wrkviz/ARCHITECTURE.md` (1,379 lines); the operator guide is
`py/wrkviz/USER_GUIDE.md`.

### Storage: a directory archive, not a database

There is no SQLite or server database. An archive is a directory (`ARCHITECTURE.md`, "Durable
project and export layout"):

| Path | What it holds | Written by |
|---|---|---|
| `teams/<slug>/raw/team.json` | The normalized model of one team (one coordinator lineage): agents, turns, events, tool calls, edges. Tool payloads are references `{sha256, byte_length}` | ingest |
| `teams/<slug>/raw/normalized-generation.json` | Generation marker, written last; readers refuse a stale or missing one | ingest |
| `teams/<slug>/raw/task-notes.jsonl` | Frozen Orc task-note records (upstream deletes them) | ingest (Orc) |
| `teams/<slug>/payloads/<xx>.jsonl` | Content-addressed tool stdin/stdout and patch bodies, gitignored | ingest |
| `<archive>.sources/<slug>/` | Snapshots of the vendor logs (the bulk: 67.5 % of one measured archive), outside the archive | ingest |
| `extracted/transcripts/*.jsonl.gz` | Provider-neutral prompt, message and system-input projections, seekable multi-member gzip with `.index.jsonl` sidecars | `extract-transcripts` |
| `teams/<slug>/summary_data/cache/<input-hash>.json`, `name_cache/`, `_usage/receipts/` | Immutable paid artifacts, content-addressed by their exact model input; one usage receipt per backend batch | `summarize` |
| `teams/<slug>/summary_data/{phases,agents,rollups/<kind>}/` | Projections: the current artifact per logical key | `summarize` |
| `data/timeline-v3.json`, `data/timeline-v3/**.jsonl.gz` | The published website data: schema 3, seekable JSONL shards per team and UTC day, a spine per team, bins, search corpus, Bloom prefilters | `build`, `export` |

The model types are frozen dataclasses in `py/wrkviz/model.py`: `Agent` (thread id, parent, path,
nickname, role, depth, lifetime), `Turn`, `Event` (kind, role, text, author, recipient,
`ingress_kind`, `author_kind`, source line), `ToolCall` (name, timing, payload references), `Edge`
(spawn, continuation, result, message), `TaskNote`, and `TeamData` which bundles them.

### Ingestion

| Source | Module | Notes |
|---|---|---|
| Codex rollouts | `codex.py` (1,934 lines) | Newline-complete prefix snapshots; reruns accept only exact reuse or prefix extension; explicit continuation chains, never inferred from time |
| Claude Code JSONL, with subagents | `claude.py` (1,320) | Discovers `<session>/subagents/agent-*.jsonl` (`claude.py:255`) |
| Claude Code prompt history | `claude_history.py` | Prompts only, kept after Claude deletes transcripts |
| Orc SQLite sessions and TaskGraph | `orc.py` (7,350) | Content-addressed SQLite backups, append guards, tombstoned task notes |
| Registered projects | `project_config.py`, `ingest-project` | A JSON config of teams per project, per host; per-team isolation of failures |
| GitHub | `github_metadata.py`, `github_refs.py`, `gh_resolve.py` | Pull-request titles for links |

There is no Muse ingester. Ingestion is from local paths; getting another host's logs onto this host
is done outside the tool (a private fetch script mirrors the agents' log directories from several
hosts over SSH into one local tree).

### Summarization levels (`summary_registry.py`, `summarize.py`, `naming.py`, `pipeline.py`)

| Registered summarizer | Unit | Staged by |
|---|---|---|
| `phase-work-summary` v2 | One agent's fixed 30-minute UTC phase (`phases.py:409`, `phase_minutes = 30`) | `pipeline.py:2570 _phase_jobs` |
| `agent-lifetime` v2 | One agent: a two-to-five-word name, rationale, lifetime summary | `pipeline.py:3237 _agent_name_jobs` |
| `project-overview` v2 | One team's knowledge epoch | `pipeline.py:2426` |
| `technical-rollup` v3 | Hour, day, week, month or quarter, from contained lower levels | `pipeline.py:3552 _rollup_jobs_for_level` |
| `plain-language-rollup` v4 | The same periods, rewritten for a newcomer from the technical result | same |
| `glossary-definition` v2 | **historical-disabled**: no new jobs; builds publish an empty glossary | — |

So the levels are phase, agent, hour/day/week/month/quarter, and project. **There is no per-message
summary in wrkviz**; the owner's recollection of "each message, each hour, each day" maps to phase,
hour and day. Each artifact records its frontier status (`project-start`, `contiguous-extension`,
`isolated-backfill`) and per-channel context coverage. Backends are the Codex CLI and `claude
--print` (no direct API client). Cache identity is the exact model payload plus contract versions,
backend, model, effort and tier.

**Measured on the owner's private 12-team archive** (60 days, 2026-07-20 to 2026-09-18; read-only
from its bootstrap and usage receipts by `/tmp` scripts on 2026-10-23):

| | Count |
|---|---|
| Events | 499,155 (9,001 owner prompts, 119,657 agent responses, 301,532 tool calls, 68,948 inter-agent messages) |
| Agents | 2,912, of which 420 named (14 %) |
| Phases | 18,371, of which 1,849 summarized (10 %) |
| Rollups | 56 hourly, 26 daily, 4 weekly, 4 monthly, 4 quarterly |
| Phase/rollup jobs | 2,425 in 1,194 batches; 43.3 M input and 3.8 M output tokens; 17.9 k input and 1.6 k output per job on average |
| Naming jobs | 640 in 242 batches; 8.9 M input, 0.27 M output |
| Single-job latency | phase via Codex CLI p50 20.2 s, p90 27.9 s; via `claude --print` p50 24.2 s, p90 58.3 s |

That is about 8,300 transcript events a day, two orders of magnitude more than the chat layer.

### Labels and classification

**Not found.** There is no label, tag or classification feature in wrkviz's code, issues or branches.
The nearest pieces are: the retired `glossary-definition` summarizer and its `audit-glossary`
command (`glossary_audit.py`, `terminology.py`, which mines slug-shaped terms with a regex, line 48),
and the "supported semantic concepts" input channel that every phase and rollup prompt reserves and
that is **empty "until the bounded semantic discovery pipeline is implemented"** (`ARCHITECTURE.md`,
"Phase work summary"). That empty channel is exactly where labels and topics would go. If the
labels work exists somewhere else (another checkout, a conversation), it has not landed here.

### Timeline website

`static/app.js` (8,458 lines), `timeline-core.js`, `index.html`, `style.css`. Loads
`data/timeline-v3.json`, reads shards one gzip member at a time over HTTP `Range`, and chooses detail
by time density: detail (phases, states, message edges) at up to one minute per pixel, lifetime
(one block per agent) to five, aggregate (one row per team from bins) beyond. Transcript search
uses per-team Bloom prefilters. Served by `standalone_server.py` (gzip sidecars, strong ETags,
single `Range`, no directory listing, refuses `source_snapshots` paths). Measured: first paint
2.16 MB transferred in 34 requests on the 12-team archive (`ARCHITECTURE.md`).

### CLI (`cli.py`)

`quickstart`, `ingest`, `ingest-claude`, `ingest-claude-history`, `ingest-orc`, `ingest-project`,
`summarize`, `build`, `extract-transcripts`, `export` (multi-team composition, `multi_team.py:880`),
`refresh`, `refresh-claude`, `refresh-orc`, `github-metadata`, `serve`, `inspect`, `gc`,
`migrate-build-state`, `migrate-snapshots`, `audit-glossary`, `audit-losslessness`, and `query
{list, show, search, prompts, messages, stats}`. Every archive also gets a dependency-free
`./timeline` CLI and `serve.py`.

## vibe-talk (`vibe-talk/`, Rust server plus one TypeScript-checked page)

vibe-talk is the chat half: a phone- and car-friendly reader for chat channels (Discord, Slack, a
Google Chat bridge) with read-aloud and a voice agent. **Its central design rule is that it is
stateless about messages**: the chat provider is the source of truth and is read on demand
(`src/store/mod.rs`, module note). Two exceptions keep message-derived data.

### The durable store (`src/store/`, SQLite, `sqlite.rs` 6,655 lines)

One file, `0600`, `secure_delete`, a `user_version` migration ladder (`sqlite.rs:48 MIGRATIONS`).
Tables: `conversations` and `turns` (voice transcripts), `read_marks`, `summaries`, `dismissals`,
`channel_aliases`, `added_channels`, `noise_rules`, `noise_exemptions`, `hidden_channels`, `pins`,
`pin_revisions`, `upstream_clear`, `bookmarks`, `voice_settings`, `voice_prompt`,
`voice_post_prefix`, `thread_hues` (`#427`, `sqlite.rs:384`). Behind the `StateStore` trait
(`store/mod.rs:1634`, about 60 methods) with a fake and a disabled implementation. **It holds no
message text** except summaries, which are keyed `(policy version, channel, message id, content
hash)` (`sqlite.rs:81`) so an edit invalidates one row and a policy change invalidates all.
Retention is enforced on every write.

### The message cache (`src/cache/`, a second SQLite file, about 21,000 lines with tests)

`#208 persistent-cache`, `#297 cache-coverage-races`, `#384 cache-serve`. A **disposable** on-disk
copy of channel messages, never attached to the store. Tables (`cache/schema.rs:52`): `account`,
`channel`, `message` (author, content, content hash, `content_partial`, thread, home scope,
`search_text`), `tombstone`, `message_scope`, `coverage` (half-open intervals with `listed_ms`,
`full_ms`, `complete`, `source`), `scope_head`, `thread`, and an FTS5 trigram index. One writer
thread applies offers in sequence; readers re-check generation and channel epoch after their SQL;
a dropped offer marks its channel suspect; access refusals erase. Defaults: 90 days, 256 MB
(`config.rs:204`). It fills from write-scope reads and the live feed; it never fills from a
read-scope caller (`cache/grant.rs`).

**It serves almost nothing yet, and on Slack that is by design.** The coordinator reports
`serve = false` on the Slack deployment with 2 of 299 evaluated reads servable. Reading the rules
in `cache/shadow.rs` explains why without assuming a bug:

1. **Windows** (`messages`, `digest`, `todo`, `bankruptcy`, `speech`) need a fresh head (listed in
   the last 15 s, 60 s with a live feed) *and* current claims under the window (`newest`,
   `shadow.rs` around line 700). `current()` passes only when the claims were re-listed inside the
   reuse period or the feed carried every edit and deletion since (`fed_since`), and `fed_since`
   requires `Feed::Deletions`, which only a bridge has: Slack's feed is `AdditionsOnly`
   (`shadow.rs`, `Feed::of`). So on Slack a window is servable only when the same stretch was listed
   upstream within the last minute, which is when the server already has it in memory. They count
   as `stale_head`. And windows on Slack are never serve-eligible anyway (`serve_eligible`,
   requirement (c): a Slack window upstream carries thread replies the cache holds only in their
   threads).
2. **Pages and counts** need complete coverage, and Slack's coverage is never complete
   (`incomplete`).
3. **A message by id or a summary's message** is servable on Slack only within two hours of its home
   scope's revalidation (`REVALIDATED_ADDITIONS_ONLY_MS`); otherwise it is counted
   `needs_existence_check`, not `would_serve`.
4. **Timeline pages** of Main are eligible, but the Threads and All views are never asked, a thread's
   page is ineligible, and a Main page needs coverage of `limit` rows down from the head plus a
   vouched band (`gap` otherwise).
5. **What fills it** is only write-scope reads and pushes. A voice agent holding the read token
   fills nothing, and after a restart the feed vouches for nothing from before it.

This is inferred from the code. The diagnostics report's per-consumer miss counts
(`message_cache.shadow[].misses`, `src/diagnostics.rs:175`) would confirm which reason dominates.
The lesson for the synthesis: **serving a read instantly and having a complete history are
different requirements.** The cache's rules are tuned for the first (never show a deleted message
as current). Summaries, timelines and reports need the second, which a periodic, verified backfill
gives without any of the serve-time freshness rules.

### Summarization (`src/summarize/`, `src/summary.rs`)

| Piece | Where | What it does |
|---|---|---|
| Per-message one-liner | `summarize/mod.rs` `PROMPT`, `messages_api.rs` | One plain Messages API call per long message (Anthropic or Vertex dialect, default Haiku 4.5), or the ElevenLabs agent as fallback. Cached in `summaries` by content hash; policy version folds prompt, model, width, context |
| Thread name and sentence | `summarize/threads.rs` | A 2-to-4-word hyphenated slug chosen once and kept (`THREAD_RULE`, at most 40 characters), plus one sentence refreshed at 1, 2, 4, 8 messages then every 10. Background queue, per-channel read budget. 339 threads named on the busiest deployment by 2026-10-22 (side-agent doc) |
| Rate limiting and counters | `summarize/rate_limit.rs`, `counters.rs` | Per-minute pacing |
| `digest` (not a summarizer) | `summary.rs`, `ops.rs:1536` | One line per message: id, local time, author, the first 160 characters of the speakable body with code and links replaced by placeholders (`condense`, `summary.rs:66`). No model, no cost |

The `digest_channel` MCP tool that failed for the owner is that `digest`: one upstream window read
with no deadline and no held copy, so behind a slow adapter it outlived the voice runtime's 15 s tool
limit. Since `#437 voice-full-text-catchup` it is unlisted (`listed: false`, `mcp/mod.rs:246`); the
REST route remains for the page. `recent_activity` now carries full text from the held recent
window with a 32 KB budget, 10 messages per channel or 30 for one channel (`ops.rs:1572-1584`),
answering within 2.5 s for a held channel and 8 s cold (`recent.rs:102-107`).

### Thread names are the topic slugs

The thread slug (`runner-wedged`, `cache-key-rewrite`) is already the shared terminology the owner
wants the voice agent to use; the side-agent proposal builds its catch-up around exactly these names.
They are per thread, though: there is no grouping of threads into workstreams, and no slug that spans
channels or reaches the transcript layer.

### Label suggestions

**Not found** in vibe-talk either: no label, tag or suggestion feature in `src/`, `web/`, the
issues (`.minibeads/issues/`) or the branches. `#242 rich-headers` lifts an agent's `[who-am-i]` tag
into the header row, and `#303 voice-post-prefix` tags what the voice agent posts; neither is a label
system. The thread-naming slug is the closest existing piece.

### Voice context today

| Mechanism | Where | Push or pull |
|---|---|---|
| System prompt, version 12 (abilities, owner rule, plain speech) | `prompts/voice-agent-system.txt`, `voice_agent.rs`, `voice_prompt.rs` | push, at session open |
| Owner identity (`[owner]`, "the user (Name)") | `owner.rs`, `untrusted.rs` | in every tool result |
| Recap of the previous voice conversation (Resuming, off by default) | `replay.rs`; bridge `replay` frame, hosted `contextual_update` | push, first frame only |
| Call-start read-ahead of every channel's window into the held copy | `recent.rs`, `read_ahead.rs` | server-side warm-up, nothing sent |
| Arriving messages relayed into a call (off, gist or full) | `web/voice.js` (`relay`, `notice` turns) | push, mid-call, as a spoken turn |
| MCP tools: `recent_activity`, `list_channels`, `read_page`, `count_messages`, `find_message`, `read_message`, `post_reply` (`ask_agent` unexposed) | `mcp/mod.rs`, `mcp/protocol.rs` | pull, 12 s deadline each |

The `vibe-talk-v1` bridge protocol has no silent mid-call context frame: `replay` is accepted once,
before the first turn (`contract.rs`, `VibeTalkV1ClientFrame::Replay`). The page can push mid-call
only as a turn (`notice`), which the agent answers aloud.

### Web app

`web/voice.html` and `web/voice.js` (51,803 lines). The top bar has **one two-position switch**,
Voice and Channel (`voice.html`, `#view-switch`); the channel view has Main, Threads and All tabs.
A third "Work" view means turning the switch into a three-way segmented control. The page already
has floating strips (the read-aloud `#read-steer` strip of `#376`/`#433`, the search glass), which
the summary-availability bar can copy.

### Guarantees vibe-talk keeps, which any merge must keep

- **Scopes.** Read token versus write token (`auth.rs`); a read-scope request writes nothing durable;
  only write-scope reads fill the cache (`cache/grant.rs`).
- **Allowlist.** Only configured or owner-added channels are read, cached or summarized; removing a
  channel sweeps it.
- **Untrusted text.** Every model prompt fences third-party text through `untrusted::fenced`, with a
  private prompt field so no call site can skip it; continuation lines are indented so text cannot
  forge a line (`untrusted.rs`, `#438`).
- **Noise rules.** Evaluated on current text at serve time, never recorded (`noise.rs`).
- **Owner identity.** The owner's messages are marked, never read back unless asked; display names
  that start "the user" are quoted (`owner.rs`).
- **Access loss erases.** A provider refusal blocks at once and erases once confirmed
  (`cache/access.rs`).
- **Privacy at rest.** `secure_delete`, FTS5 secure-delete, tombstones of ids and times only, logs
  without text (`cache/mod.rs`, `access.rs`).
- **Single tenant.** One owner per store (`store/mod.rs`).

## Overlaps, gaps and unfinished work

| Concern | wrkviz | vibe-talk | Overlap or gap |
|---|---|---|---|
| Item storage | Directory archive, JSON and seekable JSONL, durable and git-friendly | Disposable SQLite cache plus stateless reads | Two stores with opposite durability stances |
| Incremental ingest | Snapshot, prefix-extension and append guards per vendor file | Live feed plus coverage intervals and heads | Both track "how far have I seen", in incompatible terms |
| Full-text search | Trigram Bloom prefilter plus scan over seekable shards | FTS5 trigram | Same idea, two implementations |
| Model client | Codex and Claude CLIs, batched | Direct Messages API, per item | vibe-talk's client is the better base for a server |
| Summary cache identity | Content hash of exact payload plus contract versions, immutable artifacts, receipts | Policy version plus content hash, mutable rows, no receipts | Same principle; wrkviz's is stricter and auditable |
| Levels | phase, agent, hour to quarter, project | message, thread | Disjoint; together they form one ladder |
| Freshness | Frontier status, coverage percent | Sentence refresh schedule by message count | Neither answers "is hour H caught up?" |
| Labels and topics | Empty concept channel; glossary retired | Thread slugs | Unfinished on both sides; no cross-channel topics |
| Timeline UI | Full Perfetto-style site | None | — |
| Reports | Weekly plain-language rollup exists per team | None | wrkviz's weekly rollup is a report seed |
| Remote hosts | Local paths only; fetching is external | Chat is remote by nature | No transcript transport |
| Muse transcripts | Not ingested | — | Gap |
| Side-agent helper | — | Proposed only (`2026-10-22-side-agent-helper.md`) | Unbuilt |
