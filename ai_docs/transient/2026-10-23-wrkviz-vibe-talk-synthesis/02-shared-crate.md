# Appendix 2: The shared component, `logbook`

> **Update 2026-10-23 (`#455 vibe-talk-next-plan`).** Part of the master plan,
> [`../2026-10-23-vibe-talk-next/README.md`](../2026-10-23-vibe-talk-next/README.md), which links
> here for the detail of logbook's types. Terms follow its
> [terminology](../2026-10-23-vibe-talk-next/TERMINOLOGY.md). The name `logbook` is proposed for
> adoption (owner question A3).

Part of `#449 wrkviz-vibe-talk-synthesis`. `logbook` (a placeholder name when written; the master plan proposes keeping it) is one Rust crate that
owns item storage, ingestion cursors, summarization, freshness, topics, search and timeline export,
for chat messages and for agent transcripts alike. Nothing here is implemented; the sketches are
meant to be close enough to compile that the type decisions can be argued with.

## What it is responsible for, and what it is not

| In `logbook` | Stays outside |
|---|---|
| Items (a chat message, a transcript event) with precise, typed metadata | Talking to Slack, Discord or a bridge (vibe-talk's `ChatClient`) |
| Where each source has been read up to, and what the store can vouch for | Rendering a message for the page or for speech (`render.rs`, `speakable.rs`) |
| Summaries at every level, their cache identity, receipts and freshness | The voice agent, MCP tools and the page |
| Topics and labels, with suggestions and owner decisions | Read marks, pins, bookmarks, aliases (vibe-talk's own state stays in its store) |
| Search, and range and group queries | Authentication (the caller passes a capability, below) |
| Export to wrkviz's schema-3 timeline shards | The timeline browser app (static files) |

The split follows one rule: **`logbook` knows what was said and what it means; the applications
know who is asking and how to show it.**

## Layout

```
rs/logbook/            the crate: types, traits, the SQLite store, the archive reader
  src/id.rs            ids, time, spans, calendar buckets
  src/item.rs          Item<M>, Body, ContentHash, the Metadata trait
  src/meta/chat.rs     ChatMeta
  src/meta/transcript.rs  TranscriptMeta
  src/store/           Store, SqliteStore, ArchiveDir (read-only wrkviz archives)
  src/source/          Source trait; transcript sources (Claude, Codex, Orc, history, Muse)
  src/summary/         Unit, Summarizer, ModelBackend, artifacts, receipts, freshness
  src/topic.rs         Topic, Label, Labeler
  src/search.rs        Index, Query
  src/export/          timeline schema 3 writer (seekable JSONL)
rs/wrkviz/             the Rust wrkviz CLI on top of logbook (appendix 3)
vibe-talk/             depends on ../rs/logbook by path (appendix 4)
```

vibe-talk is its own Cargo workspace and its container builds from `vibe-talk/` alone
(`Containerfile`, `COPY Cargo.toml Cargo.lock ./`). A path dependency on `rs/logbook` therefore
means building the image from the repository root. That is a small change, but it is a decision for
the owner (question 2 in the main proposal); the alternative, the crate inside `vibe-talk/`, makes
the wrkviz port depend on vibe-talk's tree.

## Identity, time and spans

```rust
/// Milliseconds since the Unix epoch, UTC. Never a client clock (vibe-talk's rule).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Millis(pub i64);

/// Half-open `[start, end)`, as wrkviz's whole time model is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span { pub start: Millis, pub end: Millis }

/// Where items come from: one chat channel, or one transcript lineage on one host.
/// Rendered as `chat:<provider-key>:<channel-id>` or
/// `transcript:<host-label>:<harness>:<root-session>`; parsed, never concatenated by hand.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceId {
    Chat { provider_key: Arc<str>, channel_id: Arc<str> },
    Transcript { host: HostLabel, harness: Harness, root_session: Arc<str> },
}

/// A stable item id: its source plus the provider's own id (a snowflake, a Slack `ts`, a
/// transcript event id). Survives moving between a single-team and a combined archive, as
/// wrkviz's canonical references already do.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId { pub source: SourceId, pub native: Arc<str> }

/// A group of items that is summarized together below the calendar levels: a chat thread, a
/// channel's main timeline, or one agent (wrkviz's thread id).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GroupKey {
    ChatMain(SourceId),
    ChatThread(SourceId, Arc<str>),
    Agent(SourceId, Arc<str>),
}

/// A calendar bucket. Hours are UTC-stable keys with local labels (wrkviz's rule); days and
/// weeks start at local midnight and local Monday in `tz`, so a DST day is 23 or 25 hours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level { Hour, Day, Week, Month, Quarter }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Bucket { pub level: Level, pub span: Span, pub tz: jiff::tz::TimeZone }
```

`SourceId::Transcript` carries a `HostLabel`, an operator-chosen short label ("build-box-a"), never
a hostname, because labels end up in URLs, exports and reports.

## Items and metadata

The owner asked for a store "parameterized by a precise type for message metadata or a precise type
for agent transcript metadata". That is the `Metadata` trait: each source family brings one concrete
type, and the store is generic over it.

```rust
/// What is common to every item, whatever it is.
#[derive(Clone, Debug)]
pub struct Item<M: Metadata> {
    pub id: ItemId,
    /// When it happened, provider time.
    pub at: Millis,
    /// When the store learned this version (vibe-talk's `observed_ms`).
    pub observed: Millis,
    pub edited: Option<Millis>,
    pub body: Body,
    pub meta: M,
}

/// The text, or a pointer to it. Untrusted by construction: there is no `&str` accessor
/// that skips the fence; a model prompt can only be built through `untrusted::Fenced`.
#[derive(Clone, Debug)]
pub enum Body {
    Text { text: Arc<str>, hash: ContentHash, partial: bool },
    /// Large tool payloads live content-addressed beside the store, as wrkviz's payload tree.
    Payload { hash: ContentHash, byte_length: u64 },
    /// The provider marked it unavailable or encrypted (wrkviz's `content_availability`).
    Absent { reason: AbsentReason },
}

pub trait Metadata:
    Clone + Send + Sync + Serialize + DeserializeOwned + 'static
{
    /// Stored with every row and every artifact, so a reader refuses a family it does not know.
    const FAMILY: &'static str;          // "chat" | "transcript"
    const SCHEMA: u32;

    /// Who wrote it, in the provider-neutral classes wrkviz already uses for prompts.
    fn author(&self) -> Author;
    /// The group it is summarized in below the calendar levels.
    fn group(&self, source: &SourceId) -> GroupKey;
    /// Extra searchable text beyond the body (an author name, a tool name), or nothing.
    fn search_fields(&self) -> SmallVec<[&str; 2]>;
    /// Whether a summarizer should see it at all (noise placeholders, lifecycle records).
    fn summarizable(&self) -> Summarizable;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Author {
    Owner,                       // the [owner] accounts, or author_kind = owner_human
    OtherHuman { display: Arc<str> },
    Agent { display: Arc<str> },
    System,
    Unknown,                     // never guessed from prose (wrkviz's rule)
}
```

The two metadata types look very different, which is the point of keeping them apart:

```rust
/// A chat message: what vibe-talk's cache row stores today (cache/schema.rs `message`),
/// minus the serve-time fields it already refuses to store.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMeta {
    pub thread: Option<Arc<str>>,
    pub is_root: bool,
    pub reply_to: Option<Arc<str>>,
    pub author_id: Arc<str>,
    pub author_name: Arc<str>,
    pub author_is_bot: bool,
    pub home_scope: ChatScope,           // Main | Flat | Thread
    pub reactions: Vec<Reaction>,
    /// The agent session that posted it, when the join below found one.
    pub posted_by: Option<ItemId>,
}

/// One normalized transcript event: wrkviz's Event, ToolCall and Edge folded into one enum,
/// because they share time, agent and turn and are summarized together.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranscriptMeta {
    pub harness: Harness,                // Claude | Codex | Muse | Orc | ClaudeHistory
    pub agent: AgentRef,                 // thread id, parent, path, nickname, role, depth
    pub turn: Option<Arc<str>>,
    pub kind: EventKind,
    pub ingress: Option<IngressKind>,    // owner chat, inter-agent, scheduled, hook ...
    pub author_kind: AuthorKind,
    /// Where in the vendor file it came from: path index and byte offset or line, so a reader
    /// can open the original and an audit can account for every row (losslessness).
    pub origin: Origin,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EventKind {
    UserPrompt,
    AgentResponse,
    Reasoning { encrypted: bool },
    ToolCall { call_id: Arc<str>, name: Arc<str>, ended: Option<Millis>,
               input: Option<ContentHash>, output: Option<ContentHash> },
    InterAgent { to: Arc<str>, edge: EdgeKind },   // spawn, continuation, result, message
    TaskNote { task_id: Arc<str>, title: Arc<str> },
    System,
    Lifecycle,
}
```

Agents themselves (lifetimes, parents, names) are not items. They are a side table every
`TranscriptMeta` source maintains (`Store::groups`), because the timeline needs them without
scanning events, exactly as wrkviz's spine does.

## The store

```rust
/// One store holds one family. A deployment has a ChatStore and a TranscriptStore, possibly in
/// one SQLite file with per-family tables, possibly in two (decided per deployment).
pub trait Store<M: Metadata>: Send + Sync {
    /// Apply a batch from one source read. Idempotent: the same batch twice changes nothing.
    /// The cursor is committed in the same transaction as the items.
    fn apply(&self, batch: Batch<M>) -> Result<Applied, StoreError>;

    fn get(&self, id: &ItemId) -> Result<Option<Item<M>>, StoreError>;
    fn range(&self, q: &RangeQuery) -> Result<Vec<Item<M>>, StoreError>;
    fn groups(&self, source: &SourceId, span: Span) -> Result<Vec<GroupInfo>, StoreError>;

    /// How far each source is known, and how sure the store is (below).
    fn watermark(&self, source: &SourceId) -> Result<Watermark, StoreError>;
    /// Counts and newest instants per bucket, for the availability bar and cheap stats.
    fn tallies(&self, sources: &[SourceId], buckets: &[Bucket]) -> Result<Vec<Tally>, StoreError>;

    /// Erase a source or a scope: chat access loss, a channel taken off the list, the purge.
    fn erase(&self, what: Erase) -> Result<ErasedReceipt, StoreError>;
}

pub struct Batch<M: Metadata> {
    pub source: SourceId,
    pub read_began: Millis,              // vibe-talk's Stamp: when the upstream read STARTED
    pub items: Vec<Item<M>>,
    pub removed: Vec<ItemId>,            // deletions become tombstones of ids and times only
    pub claim: Claim,                    // what this read proves (below)
    pub cursor: Cursor,
}
```

### What a store can vouch for: `Claim` and `Watermark`

This is where the two families differ most, and where the low cache coverage on Slack comes from
(appendix 1). A transcript file is append-only: once the newline-complete prefix up to byte
`n` is read, nothing below `n` can change without the append guard refusing. A chat channel is
mutable and its feed may carry no deletions. So:

```rust
pub enum Claim {
    /// Transcripts: everything up to this cursor is held, exactly (append-only prefix).
    Prefix { through: Cursor },
    /// Chat: a listing proved these intervals of this scope at `listed`; `complete` is false
    /// for providers whose listings may omit messages (Slack's). The interval algebra is
    /// vibe-talk's cache/coverage.rs, moved here unchanged.
    Listing { scope: ChatScope, intervals: Vec<Interval>, complete: bool },
    /// A push or a by-id read: the item exists; nothing about its neighbours.
    Sighting,
}

pub struct Watermark {
    /// Every item before this instant is held (for chat: as of the latest verifying listing).
    pub complete_through: Option<Millis>,
    /// What "complete" means for this source: `Exact` for an append-only transcript prefix and
    /// for a provider whose listings omit nothing; `AsListed` for Slack, whose history listings
    /// leave out messages with no author (`cache/observe.rs`, `Shape::complete`).
    pub completeness: Completeness,
    /// The newest item held.
    pub newest: Option<Millis>,
    /// When the store last verified `complete_through` against the source.
    pub verified_at: Millis,
    /// Doubt: an offer was dropped, an access refusal is pending, and so on.
    pub doubt: Option<Doubt>,
    /// The store's change counter for this source: bumped by every applied create, edit and
    /// tombstone. Freshness compares it, never clocks.
    pub change_seq: u64,
}
```

**Two rules use these, and they must stay separate.** Serving a read instantly (vibe-talk's M9)
keeps the strict `shadow::decide` rule over `Claim::Listing`, unchanged. Summaries, timelines and
reports use only `Watermark::complete_through`, which a periodic **backfill walk** advances for a
chat source (page back through history until it meets held coverage, a few calls per channel per
hour), and which freshness checks (below). vibe-talk already declares this walk without
implementing it: `[message_cache] backfill`, default `adaptive` ("fetch older history while nothing
else is using the provider, one call at a time", `config.rs:266`), and the fill kinds
`ServerFill::Backfill` and `ServerFill::Refresher` (`cache/grant.rs`) have no caller yet. Building
them is the first step of cutover C4 (appendix 4), and more filling is also what would close the
coverage gaps behind the Main timeline's misses (appendix 1).

On Slack a walk inherits the listings' one omission (messages with no author, which are join and
system notices rather than anyone's words), so its watermark is `AsListed`: complete as far as
Slack's history lists, which is the honest meaning of "complete" for summaries there. The serve rule
keeps treating Slack as incomplete, as it must.

## Sources

```rust
/// Something that yields batches: a transcript file set, a remote host, or (in vibe-talk) the
/// chat provider's reads and live feed adapted into batches.
#[async_trait]
pub trait Source<M: Metadata>: Send + Sync {
    fn id(&self) -> &SourceId;
    /// Read what is new since `cursor`. Deterministic, spends no tokens, safe to run often.
    async fn poll(&self, cursor: &Cursor, budget: ReadBudget) -> Result<Batch<M>, SourceError>;
}

pub enum Cursor {
    /// Per vendor file: path index, byte offset of the end of the last complete line, and the
    /// digest of that prefix (wrkviz's snapshot manifest, kept as a cursor).
    Files(Vec<FileCursor>),
    /// Orc: append watermark per table plus the prefix digest (wrkviz's append guard).
    Sqlite(SqliteCursor),
    /// Chat: opaque provider cursor plus the oldest instant the backfill has reached.
    Chat { provider: Option<Arc<str>>, backfilled_to: Option<Millis> },
}
```

Incremental reading is not optional. Measured on this host on 2026-10-23: the Claude transcript files
touched in the last 24 hours were 39 files and 2.46 GB (16 of them subagent files), and Codex's were
126 files and 5.69 GB; parsing them whole took 16.6 s and 24.9 s. A cursor that resumes at the last
complete line makes a poll cost what was appended, which is what lets deterministic ingest run every
minute.

Remote transcripts are a `Source` too (appendix 5): `RemoteSource` wraps a transcript source on
another host behind either an SSH pull of appended byte ranges or a small read-only agent.

## Summaries

The levels of both tools form one ladder. A `Unit` is what one summary is of:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    /// One long chat message (vibe-talk's one-liner).
    Item(ItemId),
    /// A chat thread's name and sentence, or an agent's lifetime name and summary.
    Group(GroupKey),
    /// One agent's fixed 30-minute phase (wrkviz's phase-work-summary).
    Phase { agent: GroupKey, span: Span },
    /// A calendar bucket over a set of sources: one channel's hour, a team's day, a report's week.
    Rollup { sources: SourceSet, bucket: Bucket, audience: Audience },
    /// A project overview epoch, or a weekly report (appendix 7).
    Document { kind: DocumentKind, sources: SourceSet, span: Span },
}

pub enum Audience { Technical, PlainLanguage }
```

```rust
/// A registered summarizer: wrkviz's summary_registry entries, as types. Not object-safe (it
/// has associated consts and a type), so the registry is an enum over the registered summarizers,
/// or holds an erased `dyn StageJobs` wrapper; either way the IDs and versions stay constants.
pub trait Summarizer: Send + Sync {
    const ID: &'static str;              // "phase-work-summary", "chat-hour", ...
    const VERSION: u32;                  // bump on any model-visible change
    type Output: Serialize + DeserializeOwned + Validate;

    /// Deterministically build the model-visible input for a unit, with its dependencies
    /// (lower-level artifacts) and context coverage. No model call. May say "nothing to do".
    fn stage(&self, unit: &Unit, ctx: &StageContext) -> Result<Staged, StageError>;
}

pub struct Staged {
    pub unit: Unit,
    pub payload: Fenced,                 // only constructible through untrusted::fenced
    pub depends_on: Vec<ArtifactId>,
    pub coverage: Coverage,              // per-channel requested/provided, wrkviz's rule
    /// The store's state the payload was staged from, per source (below).
    pub staged_from: BTreeMap<SourceId, StagedFrom>,
    /// The noise rules and other serve-time predicates the stage applied, as a digest.
    pub filters_digest: Digest,
}

/// Recorded per source at staging, under the store's read transaction.
pub struct StagedFrom {
    pub change_seq: u64,                 // Watermark::change_seq at staging
    pub as_of: Millis,                   // store time of that read
    pub summarizable_seen: u32,          // items the payload included (noise excluded)
}

/// A model behind one call or one batch. vibe-talk's Messages API client becomes one impl;
/// wrkviz's Codex and Claude CLI runners become two more; a fake counts calls in tests.
#[async_trait]
pub trait ModelBackend: Send + Sync {
    fn identity(&self) -> BackendIdentity;          // backend, model, effort, tier
    async fn run(&self, jobs: &[Staged], schema: &JsonSchema) -> Result<BatchResult, ModelError>;
}

/// Immutable, content-addressed by (summarizer ID and VERSION, output schema, backend
/// identity, exact payload): wrkviz's cache identity, now for chat summaries too.
pub struct Artifact<O> {
    pub id: ArtifactId,
    pub unit: Unit,
    pub output: O,
    pub staged_from: BTreeMap<SourceId, StagedFrom>,
    pub filters_digest: Digest,
    pub coverage: Coverage,
    pub frontier: Frontier,              // ProjectStart | ContiguousExtension | IsolatedBackfill
    pub receipt: ReceiptId,              // tokens, latency, backend, model; one per batch
    pub made_at: Millis,
}
```

vibe-talk's `policy_version` becomes the artifact identity, and its per-message summaries gain
receipts. wrkviz's immutable-artifact discipline (never overwrite at a different hash; projections
point at the best compatible artifact; old versions stay readable) is kept as is.

### Freshness: is a summary "caught up" with its range?

The owner's green box border needs one precise predicate:

```rust
pub enum Freshness {
    /// No artifact for this unit.
    Absent,
    /// An artifact exists and its input saw every item the store now holds in the unit's span,
    /// AND the store's watermark proves nothing older is missing.
    CaughtUp,
    /// An artifact exists, but `new_items` arrived (or were edited or deleted) since its input.
    Behind { new_items: u32, since: Millis },
    /// Caught up with what is held, but the store cannot prove the span complete
    /// (backfill not reached, a feed gap, doubt). Shown amber, never green.
    Unproven,
}

/// The current state of a unit's span, from the store, at the moment of asking.
pub struct Held {
    pub change_seq: u64,        // highest change_seq of any item in the span (creates, edits,
                                // tombstones), per source; one entry per source in the unit
    pub summarizable: u32,      // non-noise items in the span now
}

pub fn freshness(
    unit_span: Span,
    art: Option<&ArtifactMeta>,
    held: &BTreeMap<SourceId, Held>,
    wms: &BTreeMap<SourceId, Watermark>,
    filters_digest: Digest,
) -> Freshness {
    let Some(art) = art else { return Freshness::Absent };
    // A changed noise rule changes what a stage would include: re-stage.
    if art.filters_digest != filters_digest {
        return Freshness::Behind { new_items: 0, since: art.made_at };
    }
    // Behind: anything in the span changed after the stage read it. Sequence numbers, not
    // clocks, so an edit's local time and a message's provider time are never compared.
    for (source, now) in held {
        let Some(then) = art.staged_from.get(source) else {
            return Freshness::Behind { new_items: now.summarizable, since: art.made_at };
        };
        if now.change_seq > then.change_seq {
            return Freshness::Behind {
                new_items: now.summarizable.saturating_sub(then.summarizable_seen),
                since: then.as_of,
            };
        }
    }
    // Caught up with what is held; is what is held the whole span (up to the stage)?
    for (source, then) in &art.staged_from {
        let Some(wm) = wms.get(source) else { return Freshness::Unproven };
        let needed = unit_span.end.min(then.as_of);       // the current hour: up to the stage
        let proven = wm.doubt.is_none()
            && wm.complete_through.is_some_and(|t| t >= needed);
        if !proven {
            return Freshness::Unproven;
        }
    }
    Freshness::CaughtUp
}
```

Three consequences, each deliberate:

- **Noise does not hold an hour behind.** Only `summarizable` items are counted, and a `_Working…_`
  placeholder is excluded at staging and in the count alike. A changed noise rule is caught by
  `filters_digest`, not by counts.
- **An edit or deletion makes its hour `Behind`** through the change sequence, whatever the clocks
  say. A closed hour whose summary is `CaughtUp` stays so until such a change, which is rare.
- **The current hour can be green.** Its summary needs the span proven only up to the moment it was
  staged (`then.as_of`); it stays `CaughtUp` until the next message bumps the sequence, and then is
  `Behind` until re-staged (the bar's debounce is in appendix 8). A unit over several sources (a
  report's week) is caught up only when every source is.

## Topics, slugs and labels

```rust
/// A slug: lowercase letters, digits and hyphens, at most 40 characters (vibe-talk's
/// THREAD_RULE shape, and the repository's own issue-slug rule).
pub struct Slug(Arc<str>);

pub struct Topic {
    pub slug: Slug,
    pub title: Arc<str>,
    pub sentence: Arc<str>,
    pub kind: TopicKind,               // Thread | Workstream | Term
    pub aliases: Vec<Slug>,            // a renamed or merged topic keeps its old slugs
    pub status: TopicStatus,           // Suggested | Accepted | Rejected | Merged(into)
    pub first_seen: Millis,
    pub last_active: Millis,
}

pub struct Label {
    pub topic: Slug,
    pub target: LabelTarget,           // Item | Group | Phase | Rollup
    pub source: LabelSource,           // Owner | Rule | Model { artifact: ArtifactId }
    pub confidence: Option<f32>,
}

/// Proposes topics and labels. A model labeler is a Summarizer whose output is suggestions;
/// a rule labeler is deterministic (a thread slug is a Thread topic by definition; an issue
/// slug quoted in a message, "#123 voice-page-safe-area", labels it exactly).
pub trait Labeler: Send + Sync {
    fn suggest(&self, scope: &LabelScope, known: &[Topic]) -> Result<Vec<Suggestion>, LabelError>;
}
```

Three kinds of topic, from cheapest to dearest:

1. **Thread topics** are the existing thread names: free, deterministic, accepted by definition.
2. **Term topics** are the issue slugs the repositories already use (`#449
   wrkviz-vibe-talk-synthesis`), found by an exact pattern in chat and transcripts. Free and exact.
3. **Workstream topics** group threads and agents across channels and hosts ("voice-context",
   "message-cache"). Suggested by a model in a weekly batch, shown to the owner to accept, rename,
   merge or reject. Accepted workstreams feed the "supported semantic concepts" channel every
   wrkviz prompt already reserves, which is how labels improve summaries.

Suggestions never become labels without either the owner's acceptance or an exact rule. This is the
"label suggestions" feature both tools lack (appendix 1).

## Search and access

```rust
pub trait Index<M: Metadata>: Send + Sync {
    /// FTS5 trigram (vibe-talk's index), with ANDed words and quoted phrases (#339).
    fn search(&self, q: &SearchQuery, cap: &Capability) -> Result<SearchPage, StoreError>;
}

pub struct SearchQuery {
    pub text: String,
    pub sources: SourceFilter,           // channels, teams, hosts
    pub span: Option<Span>,
    pub authors: AuthorFilter,           // e.g. exclude Owner; humans only
    pub kinds: KindFilter,               // prompts, responses, tool calls, summaries
    pub topics: Vec<Slug>,
    pub limit: u16,
    pub cursor: Option<SearchCursor>,
}

/// What a caller may see. Built by the application from its own authentication, never by
/// logbook. Every read API takes one, so there is no unguarded path.
pub struct Capability {
    pub sources: AllowSet,               // the channel allowlist, the registered teams
    pub write: bool,                     // may this read fill or refresh anything?
    pub transcripts: TranscriptAccess,   // None | Summaries | Events | EventsAndPayloads
}
```

Summaries are searchable too (wrkviz's `--scope summaries`), and a hit carries the item ids it
cites so a caller can open the original.

## Export

```rust
/// Writes wrkviz's schema 3 (timeline-v3.json plus seekable shards) for a set of sources and a
/// span. Zero tokens; byte-idempotent; removes what it does not plan (wrkviz's publisher rule).
pub trait TimelineExport {
    fn export(&self, plan: &ExportPlan, out: &Path) -> Result<ExportReceipt, ExportError>;
}
```

The schema is unchanged, so the existing browser app reads a Rust-written archive. Chat sources
export as a team whose "agents" are threads (or one lane per author), with message events and no
tool calls; appendix 5 describes the joined view.

## Joining the two layers: which agent posted a chat message

A chat message posted by a coding agent appears in that agent's transcript as the input of the tool
call that posted it (an `agentctl chat reply`, a provider CLI, an MCP `post_reply`). The text is not
always byte-identical by the time the provider lists it: Slack mrkdwn escaping and mention markup,
the `#303 voice-post-prefix` tag, and long messages split into several posts all transform it. So the
join is a **normalized match**, tried in order:

1. **Normalized hash.** Strip markup and mention syntax, a configured post prefix and whitespace on
   both sides, then compare content hashes, within a few seconds of the message's time and only
   against posting tool calls of agents mapped to that channel.
2. **Chunk match.** A message that is one chunk of a longer post matches when its normalized text is
   a contiguous slice of one posting call's input, in order with its sibling chunks.
3. **Self-tag.** Agents that sign their posts (`#242 rich-headers` lifts a `[who-am-i]` tag into the
   header row) narrow the candidate agents before matching.

Never by timing or prose resemblance alone, in keeping with wrkviz's refusal to infer lineage from
proximity: a message no step matches stays unjoined. The join rate on a real week is an M0
measurement; if it is low, the posting tools can stamp an id in an invisible field the provider
keeps (Slack `metadata`, a Discord nonce) to make the join exact going forward.

## Storage choice

**SQLite for the working store, the directory archive for durable interchange.** vibe-talk already
depends on SQLite and its migrations, privacy settings and single-writer discipline; wrkviz's
archive format is designed to be version controlled and served as static files. So:

- `SqliteStore` holds items, cursors, tallies, topics, the FTS index and artifact metadata. One file
  per deployment, or one per family; WAL; one writer thread; vibe-talk's privacy settings.
- Artifacts stay content-addressed JSON files (wrkviz's envelope), referenced from SQLite, so a
  git-tracked archive keeps working and a receipt can be audited by hand.
- `ArchiveDir` reads existing wrkviz archives directly (appendix 3) and `TimelineExport` writes the
  website.

Sizes are small: the measured 60-day, 12-team transcript archive has 499,155 events; at a few hundred
bytes of metadata per event plus text that is a SQLite file of a few gigabytes with payloads kept
outside, and chat is under 1 % of that.
