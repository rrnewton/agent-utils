# Agent Team Timeline Architecture

This document is the cost and compatibility contract for `agent-team-timeline`. It must be updated
whenever a model prompt, model-visible input, generated output field, cache identity, or summary
selection rule changes.

## Durable project and export layout

The durable Hermit archive is `~/agent_logs_archive/summary/hermit`. One archive may contain
multiple teams under `teams/<team-slug>/`; source snapshots remain ignored, while normalized data,
expensive summary artifacts, model-usage receipts, and deterministic presentation files can be
version controlled. The current Hermit archive combines the registered Codex, Claude, and Orc
coordinator sessions from all source hosts; team registration lives outside the package in the
archive project's `projects/hermit.json` configuration rather than in application code.

The durable archive and a website export are distinct concepts:

- The archive accumulates normalized team data and every validated expensive artifact.
- `extracted/transcripts/` is a provider-neutral, zero-model projection over every selected
  coordinator: `occurrences.jsonl` preserves physical provider/team occurrences, `prompts.jsonl`
  is the source-identical-deduplicated chronological prompt corpus, `messages.jsonl` adds
  mechanically turn-linked responses, and `system-inputs.jsonl` retains synthetic/scheduled inputs
  without mislabeling them as the owner's prompts. Its manifest binds every file digest and source
  generation.
- A slice export is a deterministic view over selected teams and a half-open UTC interval. It may
  contain only 2026-08-06 22:00 through 2026-08-07 07:00 EDT even though the archive retains more.
- Rebuilding an export never invokes a model. Missing summaries degrade to normalized transcript
  and statistics rather than causing unrelated cached summaries to be discarded.

That degradation is presentation-only. Missing phase, agent-lifetime, project-overview, or calendar
summary data receives an in-memory fallback labeled `Summary unavailable`, with an explicit false
availability flag in the website projection. Phase detail JSON still contains normalized transcript
and statistics, but the builder neither creates nor links summary Markdown for an unavailable
slot. It never writes a fallback under `summary_data/` or registers one as a model artifact.
Compatible phase/rollup artifacts and source-bounded lifetime artifacts take precedence. Project
overviews require their source-bound projection, and summaries whose ranges extend beyond a sliced
export are not reused. Thus patchy archives preserve verified paid work without leaking later
knowledge into an earlier slice. A later `summarize` run upgrades missing regions normally.

`build` retains its single-team archive behavior. `export` accepts repeated `--team` values and
composes those independently cached teams into one self-contained site. The compositor renders
each team without model calls, namespaces thread, phase, edge, glossary, artifact, evidence, and
detail identities by team, and aligns every track to one shared UTC interval. It merges team
project/host identity, events, statistics, rollups, summaries, and artifact catalogs while keeping
the team label on every filterable record. Repeating the same export is byte-idempotent; both
single-team and combined sites carry a strict managed-file inventory that removes only stale
presentation files. A narrower slice immediately removes details and prior shard generations that
could disclose work outside the requested range. Calendar rollups retain
the timezone in which each team's cached summary was generated; hourly keys are UTC-stable. The
export timezone controls date-bound parsing and the shared display axis, not cache reinterpretation.

The zero-token semantic-zoom projection derives hourly, daily, and weekly activity bins. Hourly
bins retain UTC-stable boundaries, while daily bins use local midnight and weekly bins use local
Monday midnight in the team's display timezone. Local calendar boundaries therefore carry their
real 23-, 24-, or 25-hour duration across daylight-saving changes. The first and last bins are
clipped to the observed archive range; time before capture began or after the latest captured
record is never presented as inactivity. Empty bins remain absent so true inactive gaps stay
visible.

Normalized providers do not expose equally precise timing. Codex and Claude commonly retain turn
and tool spans, while an Orc task note is only point evidence and its one-second normalized turn is
not a measured duration. Bins therefore retain the exact active/tool metrics for compatibility,
but the outer visualization uses separate, explicitly inferred fields. Each transcript entry
contributes the same centered five-minute evidence interval; their union determines activity
evidence opacity, and per-agent unions determine estimated average and peak worker presence. This
keeps the overview comparable across harnesses without presenting point surrogates as exact
wall-clock duty cycle. Tooltips label the values as estimates and expose the underlying evidence
count.

The browser chooses detail by time density, not by record count. At at most one minute per pixel,
the detail level renders phases, state strips, and applicable message edges. Above that and through
five minutes per pixel, the lifetime level suppresses phases/states and retains one block per
agent. Structural fork/join edges are shown there only for the selected agent family. Beyond five
minutes per pixel, the aggregate level
suppresses agents, phases, states, and edges entirely and renders one compact row per team from the
precomputed bins. It chooses the narrowest hourly/daily/weekly resolution whose nominal bin is at
least eight pixels wide. Coordinator and worker evidence is one downward-extending block:
log-scaled height encodes estimated average worker presence, opacity encodes activity-evidence
coverage, and a distinct hue marks ranges with cached summaries. Teams sort by descending recorded
event volume. Omitted zero-activity bins preserve gaps. These thresholds and suppressions are part
of the performance contract and are covered by browser tests.

### Static delivery and schema compatibility

Website builds keep timeline schema 1 and every uncompressed identity file intact. They also create
byte-idempotent gzip-6 sidecars for browser-facing JSON, JavaScript, CSS, HTML, and rendered
Markdown files of at least 1 KiB. The gzip header has no source filename and a zero timestamp, so a
content-identical rebuild does not change the sidecar or its mtime. Website exports inventory
their managed sidecars; ordinary obsolete sidecars are removed with the corresponding identity
files, while content-addressed timeline objects follow the retention rule below.
When a compressible identity changes, its old sidecar is removed before the identity is replaced,
so the server can fall back to the new uncompressed bytes rather than pair them with old compressed
bytes. All artifact catalogs, phase details, summaries, and their refreshed sidecars are complete
before a schema-2 bootstrap publishes objects that refer to them.

Builds additionally project schema 1 into the backwards-compatible schema-2 browser layout.
`data/timeline-v2.json` is the stable bootstrap: it carries site/team identity, the complete time
range, aggregate activity bins, content-addressed global and phase-index references, and a UTC-day
detail catalog. The lightweight phase index supplies complete lifetime phase cards without their
state arrays. The immutable global object carries agent lifetimes, precomputed activity bounds,
structural spawn/continuation/result edges, rollups, glossary and summary-file metadata. Each
non-empty UTC-day object carries
intersecting phases (including their states), events, and detailed message edges. A phase or
detailed edge crossing midnight appears in both day objects; the browser deduplicates it by stable
ID. Object basenames are their complete SHA-256 digest, and their gzip sidecars are generated
before the bootstrap atomically publishes those URLs. Schema 1 remains the archive-local CLI
projection and the browser fallback for older exports.
`data/timeline-v2/manifest.json` inventories the current and immediately preceding distinct object
generation. Identical rebuilds preserve that preceding generation without file churn; a later
changed generation collects objects retired for two generations. This bounded grace covers the
ordinary publication race without unbounded disk growth. Scope narrowing or a team-set change
disables the grace and purges dereferenced objects immediately so an export cannot retain data
outside its declared slice.

The browser does not infer complete statistics from a partially loaded set of days. It uses the
global aggregate only for an unfiltered full-range view, computes narrower totals only when every
overlapping day is resident, and otherwise marks event counts unavailable. Opening an agent
lifetime loads the phase index once rather than every intersecting day. Phase, agent, and rollup
zoom use precomputed exact activity bounds and do not first materialize a whole lifetime;
subsequent timeline views and modal opens reuse the same in-memory promises and objects.
Successful object requests remain cached for the page lifetime, while a rejected request is
evicted so a later user action can retry a transient server or network failure.

Both the installed `serve` command and generated `serve.py` use the same dependency-free handler.
It selects a `.gz` companion only when `Accept-Encoding` permits gzip, retains the original MIME
type, and emits `Vary: Accept-Encoding`, the selected representation's exact `Content-Length`, and
a strong SHA-256 ETag. Mutable generated names—including `manifest.json`, `index.html`, and the
schema-1 `data/timeline.json` monolith—use `public, no-cache`: browsers may store them but must
revalidate before reuse. Only a filename containing a complete 64-hex content digest receives a
one-year immutable policy; phase IDs are stable identities rather than content hashes and therefore
remain revalidated. The browser uses normal fetch caching, allowing a matching ETag to produce a
304 response. The built-in handler disables directory listing so generated object and transcript
filenames cannot be enumerated through the server.

The browser first probes schema 2, then falls back to schema 1 when the bootstrap is absent (or when
an incomplete schema-2 publication cannot be read). At aggregate zoom it uses bootstrap activity
bins without loading a detail day. Detail zoom loads only UTC-day objects intersecting the visible
range plus a one-hour-or-eight-percent buffer. Lifetime zoom uses the global lifetimes and
structural edges without loading detail days. A `Map<URL, Promise>` retains each object request,
and a second range visit never refetches or reapplies it. Full transcript search does not scan phase
detail objects. The bootstrap instead catalogs content-addressed `(team, UTC day)` search objects
whose records come directly from normalized events and condensed tool calls. The browser validates
each object's team, range, record count, record identities, roles, and known agents before merging
it. Current objects carry identity byte counts and are SHA-256 verified before JSON parsing. This
keeps search complete outside the viewport and makes inclusion independent of phase segmentation.
Each catalog entry also carries a deterministic Bloom filter over whitespace-compacted UTF-8 byte
trigrams after ASCII-only case folding. Exact search uses that same portable ASCII-only case-folding
contract; non-ASCII characters remain exact. Smart-query terms are tested independently; non-ASCII
or shorter-than-three-byte terms never reject an object, so a query with no eligible term scans all
selected objects. The filter may fetch an extra object but can never authorize a match or omit one
from a correctly built catalog. Compact content-addressed linkage sidecars preserve prompt excerpts
and response counts independently of text-shard selection, so a response after midnight still opens
its previous-day prompt. Normal typing is debounced, shard fetches have bounded concurrency, and a
request-generation guard prevents a stale search from replacing newer results.

This delivery layer is additive. Opening the archive through another static server still serves
the identity files correctly, while older generated sites without sidecars or schema 2 remain
valid. The schema-1 monolith remains available for compatibility, but schema-2 startup avoids its
whole-file transfer, parsing, and object-graph cost. Measurements and remaining work are recorded
in `ai_docs/PAYLOAD_SCALING.md`.

### Read-only query boundary

The installed `agent-team-timeline query` command and the archive-local `./timeline` Python
entrypoint read the deterministic presentation projection rather than model-cache internals. The
archive entrypoint is dependency-free, resolves its default archive from its own directory, and is
the single generated archive-local query launcher. Older archives may still contain the retired,
byte-identical `query.py` alias; readers remain compatible with it, while the next presentation
rebuild safely removes the manifest-owned alias and its narrowly named bytecode cache. The CLI's
only inputs are
`data/timeline.json`, the generation-matched schema-2 bootstrap and immutable objects, referenced
`data/details/*.json`, and referenced rollup Markdown beneath the selected archive root. Search
objects are accepted only when the bootstrap digest, team set, and time range match the loaded
timeline generation. Path resolution fails closed on absolute or escaping references. Querying has
no write path and can never invoke a model.

When schema 2 is present, the CLI reconstructs its small query projection from the bootstrap,
content-addressed global object, and phase index instead of parsing the schema-1 timeline monolith.
The global, phase-index, and search-day objects carry additive source-generation bindings; readers
accept older immutable objects that predate those fields but reject a present mismatch. Transcript
search validates and consumes one day object at a time, retaining actual matches rather than
materializing the full corpus. Compact per-day linkage sidecars supply candidate-proportional prompt
excerpts and response counts even when Bloom filtering skips the corresponding text day. Older
search-enabled archives without those sidecars retain a correct full-scan fallback. A bootstrap that
specifically predates the phase index falls back to schema 1; malformed or digest-mismatched
schema-2 data fails closed.

Availability fields are additive to timeline schema 1. Older projections without them are treated
as available; new projections mark sparse agent/phase records with `summary_available` and each
calendar audience with `technical_summary_available` or
`plain_language_summary_available`. `show` and summary search never open a Markdown path whose
audience is explicitly unavailable, while phase transcript queries remain independent of summary
availability.

The `prompts` and `messages` query actions are an independent read-only boundary over
`extracted/transcripts/`; they do not require `data/timeline.json`. The loader verifies the schema
and SHA-256 of all managed JSONL against `extracted/transcripts/manifest.json` before returning a
record. Prompt ordinals are 1-based inclusive chronological indexes for convenient slicing. Stable
`record_id` values are the durable identity because discovering older history can change ordinals.
Prompt queries default to `human`, defined only by `author_kind` values `owner_human` and
`other_human`; `bot` is `agent` or `system`, while `all` retains unresolved classifications. No
message-text heuristic participates in this boundary. Message queries apply the same prompt
selection before following `in_reply_to_prompt_id` links to responses.

The archive-local `stats` action joins that digest-verified transcript projection with the
presentation timeline through the same fail-closed readers. It has no cache, write, or model path.
It counts logical prompts by their durable human, bot/agent, or unattributed authorship label,
linked and unlinked response records, and only genuinely available summary text; sparse fallback
prose contributes availability slots but no summary words or bytes. It never guesses authorship
from message prose. Summary text is the lifetime paragraph, phase phrase plus paragraph, project
overview, or referenced rollup Markdown. Team and half-open time filtering use the normal query
rules. Whole-project overviews are excluded whenever a time bound is active because they have no
interval to overlap.

### Mechanical transcript extraction contract

`extract-transcripts` runs beneath the same archive writer lock as ingestion. It cannot reach a
summarizer backend and records zero model calls/tokens. Occurrence IDs bind team/fork namespace,
provider, thread, native source identity, source location, timestamp, and content digest; logical
IDs omit the team occurrence namespace so byte-identical declared fork history can be recognized
without being destructively content-deduplicated. Multiple text blocks from one provider message
are grouped only by their native record identity/source occurrence.

Each rerun takes a monotonic union with the prior validated generation. Records absent from the
new provider snapshot are carried forward, protecting the archive from upstream rotation or
history rewriting. A repeated native ID with different content receives a distinct occurrence
because content identity participates in the ID. Immutable occurrence fields may not change;
classifier fields and prompt ordinals are projections and may improve. Data files are written
first and the digest manifest last, so readers fail closed on an interrupted generation.
If the current provider snapshot refines the same byte-identical source occurrence from prompt to
system input (or another message class), that current classification supersedes the stale class
instead of retaining both projections. The exporter matches the full provider/source/content/event
identity without message class, verifies every other immutable field, and records the number of
such refinements. A record absent from the current snapshot is still carried forward unchanged.

Provider provenance is deliberately not reduced to `role=user`. Codex counts paired native
`event_msg/user_message` occurrences, not duplicated response-item context. Claude retains typed
and queued human-origin commands while classifying hooks, task notifications, compact summaries,
and recurring inputs as system input. Orc retains `user_source`-derived ingress and author class:
owner GChat is distinct from inter-Orc traffic, while Web/TUI submissions without an author ID
remain explicitly unknown rather than being asserted to be owner-authored. Orc also propagates a
later explicit `is_owner=true` observation backward to older GChat records carrying the same sender
identity; this is an identity join, not a prose heuristic.

Some legacy transports did not persist a sender at all. A registered team may therefore declare
strict `prompt_authorship_rules` matching its slug, ingress kind, optional half-open RFC3339
interval, and optional exact native message IDs. Rules can only refine `unknown` or
`external_or_unknown` source labels; they cannot overwrite intrinsic provider attribution.
Overlapping rules and unknown teams fail the extraction. The exporter preserves the source label,
rule ID, and human-written audit reason on each corrected record and persists the normalized rule
set as `extracted/transcripts/authorship-rules.json`. A direct later `extract-transcripts` reuses
that set; a subsequent registered-project ingest replaces it with the config's complete rule set.

### Registered project ingestion contract

`ingest-project --config FILE` is the durable orchestration boundary above the three provider
importers. Its JSON schema version is independent of provider source-manifest versions. Schema 1
requires a relative output path and an ordered, nonempty team registry. Top-level timezone,
project identity, execution hosts, and optional date/time window are inherited by each team unless
that team supplies an explicit replacement. Relative output and provider source paths resolve from
the config file's directory, making the command independent of the caller's working directory.

Every object is an exact schema: unknown and missing fields fail before provider ingestion begins.
The Codex source variant contains a sessions root, coordinator root UUID, and optional explicitly
ordered continuation UUIDs. The Claude variant contains its canonical coordinator JSONL. The Orc
variant contains its archived state root, coordinator session UUID, and optional ordered
continuations. An Orc continuation is either a whole-root UUID string or an exact
`{session_id,start_message_id}` object for a reused root whose earlier history is unrelated. Team
slugs are unique and provider-neutral; identity values pass through the same URL, hostname, and
IANA-timezone validators as individual ingest commands. Optional per-team prompt-authorship rules
are part of the exact versioned config and its receipt digest; they never inspect message text.

Configured teams run in file order through the existing provider snapshot transactions. Successful
earlier teams and their provider source manifests remain durable if a later provider fails; the
top-level receipt marks the configured run failed, and a rerun resumes idempotently. Only after all
selected ingests succeed does the command invoke global transcript extraction. Extraction always
selects every normalized archive team, even when `--team` limited this run's provider refresh,
because the monotonic prompt projection cannot omit previously exported teams.

The command has no path to summary backends and records `model_calls: 0`, `model_tokens: 0`, and
`website_build_performed: false` in its mechanical receipt, together with the exact config-file
SHA-256. It does not call `build`; presentation regeneration remains an explicit, separately
auditable zero-token operation.

Canonical references are `team:<slug>`, `agent:<team>::<source-id>`,
`phase:<team>::<phase-id>`, `rollup:<team>::<kind>::<start-ms>`,
`message:<team>::<event-id>`, and `tool:<team>::<call-id>`. The query loader strips the
multi-team compositor's `<team>::` namespace from source and phase IDs before constructing those
references. Consequently, a reference remains stable when the same team moves between an
individual archive and a combined export. List projections expose references and concise metadata;
`show` resolves relationships, optional phase transcripts, and mechanical prompt/response links.
The phase-independent transcript index contains every nonempty normalized textual event, including
system and lifecycle records, plus condensed tool records without raw tool payloads. Inter-agent
instructions and final replies are classified directionally from explicit phase and route metadata;
same-thread prompt association uses provider turn identity and `(timestamp, source line, stable ID)`
ordering. Cross-rollout parent/child returns link only to a strictly earlier instruction because
equal timestamps have no causal ordering across files. Routed prompts and responses are attributed
to the child that received or produced the work, while ambiguous routes remain on their physical
event thread.
Search objects are split by team and UTC day, content-addressed, and bound to the timeline source
generation. Parent/child route semantics link receiver-side Codex `FINAL_ANSWER` deliveries back to
their child instruction even though the two records live on different rollout threads. Both the
browser and archive-local CLI use the same catalog Bloom algorithm only as a false-positive
prefilter, then apply the same ASCII-case-insensitive/non-ASCII-exact matching contract to full
record text. JSON responses use query schema version 1, JSONL emits one record per line, and
Markdown is presentation-only.

## Codex snapshot and continuation contract

Codex ingestion snapshots the newline-complete prefix of every selected rollout before parsing.
The schema-1 source manifest records each copied path, byte length, line count, digest, and update
time. Reruns accept exact reuse, a byte-for-byte prefix extension, or a newly discovered descendant;
disappearance, truncation, prefix rewriting, source-identity changes, and unsafe/symlinked paths
fail before an existing snapshot is replaced. Parsing is restricted to the paths in that validated
manifest generation, so an orphan copy from an interrupted attempt is not adopted.

A logical coordinator history may cross process/session restarts, but this relationship is never
inferred from temporal proximity. The only authority is an ordered, repeatable
`--continuation-session <root-uuid>` argument. The first configured successor links to
`--root-session`; every later one links to the preceding configured successor. Each successor must
be a coordinator root, and thread identities may not occur in more than one configured lineage.

The first successful snapshot freezes each boundary in optional schema-1
`continuation_sessions` records: predecessor and successor IDs, both source paths, the exact final
predecessor source line and UTC timestamp before the successor start, the successor start, and the
millisecond gap. Later appends never move that boundary. A manifest without the optional field is a
fully compatible single-session archive. Once records exist, omitting continuation flags reuses
them; supplied flags must preserve the recorded ordered prefix and may only append successors.
Changing, removing, or reordering the chain requires another archive.

Canonical-lineage IDs and cache-visible inputs remain byte-for-byte unchanged. A continuation
lineage namespaces event, turn, tool, call, and fallback IDs with its root UUID and prefixes agent
paths beneath `/root/continuation-<root-uuid>`, preventing common per-session IDs and `/root` paths
from colliding. The successor coordinator keeps its own thread UUID and is parented to the preceding
root for navigation. The provider-neutral graph contains one `continuation` edge from the frozen
predecessor point to the successor start. It is structural in the site, but it is not a spawn and
the renderer does not invent a child-to-parent lifetime return or progress messages for the
successor coordinator.

## Orc snapshot transaction and compatibility contract

Orc ingestion has two identities for every source. The logical identity is its original
archive-relative path, such as `.orc/sessions/<session-id>/session.db` or `.tg/<name>.db`; this is
what discovery, ownership, and manifests use. The physical identity is an immutable,
content-addressed copy beneath `source_snapshots/.objects/<sha-prefix>/<sha>.db`. Frozen task-note
provenance is a separate canonical JSON object beneath
`source_snapshots/.projections/<sha-prefix>/<sha>.json`. Temporary SQLite backups and projection
files exist only beneath `source_snapshots/.staging/` and are never valid inputs by mere presence.

The transaction is deliberately ordered:

1. Discover one exact indexed session subtree. If no usable index exists, inspect only the
   explicitly selected root session; unrelated session-directory debris is never adopted.
2. Back up every live SQLite source into staging, run SQLite integrity/schema checks, validate all
   append-only and mutable-projection invariants, and re-read staged TaskGraph references to catch
   discovery/snapshot races.
3. Hard-link validated staged files into immutable content-addressed stores and fsync the files and
   containing directories. An orphan object from an interrupted attempt is harmless and reusable.
4. Normalize only that validated source set, then durably write the source manifest, artifact
   catalog, provider-neutral team data, and related raw projections.
5. Write `raw/normalized-generation.json` last. It binds the canonical source-manifest digest,
   byte digest of `team.json`, artifact-catalog digest, normalizer schema, and semantic source
   digest. Readers reject a missing or stale marker, so a crash between earlier writes cannot
   expose a mixed generation.
6. Only after the marker is durable, garbage-collect unreferenced managed objects. A retry removes
   stale managed staging candidates and reconstructs or reuses the generation idempotently.

Modern Orc databases retain a historical `content_blocks` prefix while new records first appear in
`messages`. Normalization is a stable block-ID union: existing content-block events keep their
canonical IDs, exact overlaps are deduplicated, and modern-only blocks extend the transcript. The
append guard and semantic source identity cover both tables. This is essential because either table
can otherwise change normalized text without changing the other table's digest.

Orc continuations are explicit-only and ordered. A whole-root successor freezes the predecessor's
last source record, the successor start, and their gap. A bounded successor additionally freezes a
source-native message ID and resolved row inside a reused root. Source discovery and normalization
apply that boundary before summary inputs are formed: inactive pre-boundary descendants and
unrelated task databases are excluded, retained agent lifetimes are clamped, and task notes shared
across roots are partitioned at the frozen boundaries. Source-manifest schema 4 records bounded
links; schemas 2 and 3 remain readable. The Orc normalized-generation schema is versioned
separately so a binary that changes these normalization rules refuses stale generated data until a
mechanical reingest repairs it.

Schema-v2 manifests and task projections are exact schemas: unknown and missing keys, malformed
digests, noncanonical projection JSON, duplicate note IDs, unsafe relative paths, symlinked path
components, and content-address/path mismatches fail closed. This applies to both live `.orc`/`.tg`
paths and archive-local object/projection paths.

TaskGraph discovery follows provider semantics rather than filename scans. A provider-initial
session (`parent_id` absent) uses its nonempty `db_name` as the primary task database, falling back
to its session UUID. Every delegated session uses its UUID even if it inherited a `db_name`.
`associated_dbs` contributes an additional union of references. A never-observed reference whose
file does not yet exist is lazy and omitted. A previously snapshotted file that remains referenced
but disappears is corruption; a source that is deliberately dereferenced is retained as an
immutable `detached` source so old history does not vanish. A later replacement receives a stable
per-owner source ordinal: ordinal zero preserves historical event IDs, while later sources use an
`-sN-` namespace. Shared-source ownership is sticky after first observation; initial ranking favors
the effective primary reference, then the provider-initial session, shallower lineage, earlier
creation time, and stable session ID.

Semantic cache identity is recomputable from the current validated generation, not a hash chain.
For sessions it includes immutable session lineage/name semantics, authoritative content-block
history, and stable AgentBlock spawn facts. `updated_at`, `db_name`, and associated-database
references are discovery/provenance and do not invalidate summaries. Conversation-state rewrites
are accepted only when every prior stable spawn fact remains an unchanged subset; other auxiliary
message churn is recorded as degraded rewrite provenance without changing normalized semantics.
Session names remain semantic because they are user-visible labels.

Each normalized source records a canonical semantic digest independently from its raw storage
prefix. `semantic_sha256` may temporarily preserve an older paid-cache key only when an exact
content-addressed `semantic_alias_baseline_path` authenticates that key and the baseline's canonical
semantics equal the current canonical digest. Task sources do not use semantic aliases. This lets
storage-only materialization and ignored provider noise reuse valid summaries without permitting a
forged or stale cache key to survive a semantic change.

For tasks, a cumulative content-addressed projection freezes `(id, task_id, content, created_at)`
plus the first observed server author, task owner, and title. TaskGraph can hard-delete a task and
cascade-delete notes, so absence from a later live database becomes a retained tombstone rather
than history loss. Overlapping immutable fields must remain exact; a new ID must exceed both the
frozen maximum and the prior SQLite allocation sequence, including trailing deleted-ID gaps.
SQLite sequence regression or ID reuse fails closed. Presence/missing hashes and rewrite metadata
audit the live observation separately from cumulative semantic identity.
Mutable enrichment drift does not silently rewrite old events or invalidate summary caches. A
non-null server author is external/server-authored provenance: it becomes an `external_message` on
the coordinator, is counted separately from user prompts, and creates no agent or message edge. A
local note with no owner is retained under a synthetic `Unattributed Task Work` child rather than
being dropped.

Legacy schema-v1 sources keep their exact historical raw-byte source-digest shape and validate the
preserved semantic-baseline object. An unchanged migration remains in `legacy-raw-v1` mode, keeping
existing summary keys byte-for-byte stable. The first real semantic change transitions that source
to deterministic `normalized-v2`; a mixed archive is supported. Normalized agent lifetimes never
use mutable session `updated_at`: they end at attributed events/tools/turns/spawns, and descendant
ends propagate recursively so every parent contains all recorded child activity.

## Cost boundary

Fetching, SQLite snapshotting, normalization, artifact extraction, terminology candidate scanning,
phase construction, statistics, Markdown/JSON generation, and site rendering are deterministic
compute. They may be rerun freely.

Six model-computation contracts are registered: five active and one retained as
`historical-disabled` so old receipts remain interpretable. Their authoritative registry is
[`summary_registry.py`](summary_registry.py). Prompt version, output schema version, known version
history, inputs, outputs, and supported granularities live there. Prompt builders import those
versions; a prompt constant must not be invented elsewhere.

| Registered summarizer | Version | Lifecycle | Unit | Prompt generator |
|---|---:|---|---|---|
| `phase-work-summary` | 2 | active | one agent phase | `summarize.py:build_summary_prompt` (`phase` branch) |
| `agent-lifetime` | 2 | active | one agent lifetime | `naming.py:build_agent_name_prompt` |
| `project-overview` | 2 | active | one project/team knowledge epoch | `summarize.py:build_summary_prompt` (`project-overview`) |
| `glossary-definition` | 2 | historical-disabled | one mechanical candidate | retained only to interpret old artifacts |
| `technical-rollup` | 3 | active | hour, day, week, month, or quarter | `summarize.py:build_summary_prompt` (`technical-rollup`) |
| `plain-language-rollup` | 4 | active | hour, day, week, month, or quarter | `summarize.py:build_summary_prompt` (`plain-language-rollup`) |

Hourly rollups use UTC-stable keys and local-time labels, including distinct keys for a repeated
daylight-saving hour. Summary selection can request only hourly work or combine it with higher
calendar levels.

### Phase work summary

Staged by `pipeline.py:_phase_jobs` after `phases.py:build_phases`.

Inputs:

- A fixed UTC phase (30 minutes by default) of one agent's messages and one-line tool aggregates,
  capped at 30,000 characters by retaining the front and back.
- Up to 16,000 characters immediately before the phase from that agent and every recorded
  ancestor, including the coordinator. This is a character budget, not currently a word budget.
- Supported semantic concepts available before the phase ends (empty until the bounded semantic
  discovery pipeline is implemented).
- User-prompt, response, inter-agent-message, and tool-call counts.

Outputs are an at-most-80-character phrase, hover paragraph, and timestamped substantive work
bullets. Current projections are `summary_data/phases/<phase-id>.json`; immutable cache records are
`summary_data/cache/<input-hash>.json`.

Work-bullet timestamps belong to the phase's half-open `[start_ms, end_ms)` interval. There is one
narrow response-recovery rule for this contract: after every other response field and bullet
shape/type has validated, bullets outside that interval are dropped rather than clamped or moved.
If any valid bullets remain, phrase and paragraph are deterministically derived only from their
texts; if none remain, the existing transcript-only heuristic supplies all three resolutions and
never reads `prior_context`. A genuinely empty model list is distinct and retains the model's
phrase and paragraph. The exception does not apply to rollups or any other malformed output, which
still fails the batch. Because this is deterministic validation of the same response contract, it
does not change prompt version or cache identity.

Important correction to earlier design recollections: phase jobs currently receive ancestor raw
transcript context and the glossary. They do **not** receive arrays of prior daily and weekly
summaries. Prior calendar summaries are inputs to calendar rollups only.

### Agent lifetime name and summary

Staged by `pipeline.py:_agent_name_jobs` only after phase summaries exist.

Inputs are the official path, coordinator nickname, role, depth, parent path, the first selected
phase's ancestor context, and all available phase-summary phrase/paragraph/bullets for the agent.
Outputs are a hindsight short name, naming rationale, and one-to-three-sentence lifetime summary.
The independent cache is `summary_data/name_cache/<input-hash>.json`; projections are
`summary_data/agents/<thread-id>.json`.

The structured-output schema asks providers for `Coordinator` or a two-to-five-word display name
with no slash or underscore. The job-aware validator remains authoritative. If a provider still
returns a path or underscore slug, one narrow presentation-only recovery takes its final semantic
component, replaces underscore separators with spaces, and revalidates the normal word-count,
length, `/root`, and path/slug rules. It does not invent or truncate words; a leaf that cannot pass
the original display-name contract still fails closed. A repaired batch retains every validated
name, rationale, and lifetime summary and records the exact raw response and replacement in the
receipt-linked backend-output audit. This deterministic enforcement of the existing output
contract does not invalidate older valid name caches.

### Project overview

Staged by `pipeline.py:_project_overview_job`.

The input is at most 48,000 characters from early root user/assistant conversation before a frozen
knowledge cutoff. Source event IDs, bounds, and a context digest are retained. The output is either
an evidence-supported newcomer overview or an explicit `Insufficient evidence` result. It may not
emit links. The projection is `summary_data/project_overview.json`. Ordinary later append-only
growth reuses the frozen evidence epoch. If a later importer discovers additional early events or
corrects a provider-system envelope that was formerly classified as an owner prompt, the pipeline
first reconstructs every recorded event by ID and proves its text, timestamp, and aggregate digest
are unchanged. Only then does it create a deterministic replacement knowledge epoch; both the old
and new content-addressed artifacts remain in the catalog. Removal, truncation, or mutation of any
recorded event still fails closed.

### Glossary definition

The definition-only contract is disabled: it established whether a mechanically selected string
could be explained, not whether the string belonged in a durable project ontology. New summarize
runs neither inject candidate strings into prompts nor launch definition jobs. Historical schema-3
projections, cache artifacts, and receipts stay untouched for provenance; builds ignore them and
publish an empty glossary until a bounded semantic discovery pipeline is implemented.

### Technical calendar rollup

Staged by `pipeline.py:_rollup_jobs_for_level` in chronological order for selected hourly, daily,
weekly, monthly, and quarterly levels.

Inputs are fully contained lower-level summaries, uncovered phase summaries at calendar
boundaries, up to ten already-completed earlier summaries of the same level, supported semantic
concepts (currently empty), and aggregate statistics. Thus a weekly job consumes daily summaries plus up to ten prior
weekly summaries; it does not directly receive an arbitrary independent array of ten prior days and
ten prior weeks. An hourly job consumes phases. A daily job consumes hourly summaries when that
level was requested in the same run, otherwise phases, plus up to ten prior daily summaries.

Outputs use the common phrase/paragraph/work-bullet schema and are projected under
`summary_data/rollups/<kind>/<key>.json`.

### Plain-language calendar rollup

This is a separate paid rewrite, not deterministic formatting. It receives the same-period
technical result as its authoritative factual account, plus analogous lower-level and up-to-ten
prior same-level plain-language results, the project overview, and supported chronological glossary
definitions. The plain result must preserve the technical result's completion state, outcomes,
counts, and time scope: for example, it cannot turn validated or approved work into landed work or
attribute an earlier backlog reduction to the current period. It identifies the project for a
newcomer and describes content before opaque work-management identifiers. It shares each rollup
projection with the technical result but has an independent input hash and usage receipt, with the
technical artifact recorded as a provenance dependency.

## Common cache identity and receipts

Both runners content-address a job over the exact structured model payload plus registered
summarizer ID/version, output schema version, backend, model, prompt version, reasoning effort, and
non-default service tier. The default tier is omitted from the hash for compatibility with caches
predating explicit tier support. Batch size and worker count do not affect artifact identity.

Validated batches are published immediately in the shared version-1 envelope defined by
`summary_artifacts.py`. Every artifact records its deterministic artifact ID, logical key, team and
time interval, complete summarizer contract, model selection, context coverage, dependency keys,
generation time, and generating usage receipt. Every attempted backend batch has an immutable
usage receipt under the relevant cache's `_usage/` tree. The envelope version describes storage
shape and is distinct from the registered summarizer and output-schema versions.

The Codex runner uses an ephemeral read-only workspace. The Claude runner uses `claude --print`
with safe mode, no session persistence, no tools, `dontAsk` permissions, and an inline JSON schema.
It accepts only the CLI's `structured_output` object and requires the native usage object on every
successful call; there is no plain-result, model, or backend fallback. Claude's normalized input
counter is direct input plus cache-creation input plus cache-read input, with both cache categories
also retained separately. Both runners feed the same strict summary/naming parsers and immutable
receipt/cache lifecycle.

When a backend's final message fails validation, or a bounded phase/name rule repairs it, the exact
raw final message is preserved at `_usage/backend_outputs/<receipt-id>.json`. Every audit record
includes a SHA-256 content hash. A phase audit also records each job's half-open bounds plus the
index, timestamp, and `dropped-out-of-range` action for repaired bullets; a naming audit records the
job key, response index, rejected path/slug, and validated display-name replacement. Audit records
deliberately exclude the model prompt and captured Codex CLI stdout/stderr. For a nonzero backend
exit, the durable failure receipt and propagated exception retain the exit code but no captured
stream detail. Ordinary valid responses need no duplicate raw-output record.

The resolvers also try the former hash scheme. Valid summary-cache v1/v2 and naming-cache v3 files
remain hits without token spend; they receive in-memory provenance marked `legacy_storage: true`
and `unknown-legacy` context coverage. They are never rewritten merely to migrate storage.
Pre-lifetime naming-cache v2 remains incompatible because it lacks the required lifetime summary.
Compatibility rule: an older paid cache whose final bullet is exactly `end_ms` remains readable as
a cache hit and byte-for-byte unchanged. Only newly validated backend output and newly generated
deterministic fallback use half-open bounds.

Projection records now carry the common provenance when it is known, while readers continue to
accept pre-envelope projections. `summary_data/artifacts.json` retains every observed artifact
reference across runs and reports counts by summarizer version, output schema, model, and frontier
status. Its selection API prefers the strongest registered compatible version and context score,
with an optional explicit model preference.

## Standard staged-computation contract

Every registered summarizer will use the following common lifecycle:

1. **Stage input.** Materialize a deterministic job with logical key, team, half-open interval,
   registered summarizer ID/version, exact model-visible payload, dependency artifact IDs, and
   context-availability metadata.
2. **Hash.** Hash the canonical prompt contract, exact payload, backend/model/effort/tier, and
   output schema. Derived diagnostic metadata that is not model-visible is recorded but does not
   masquerade as a prompt input.
3. **Resolve.** Select an exact cache hit or submit a bounded batch. Validate every response against
   that registered output schema before publication. The phase-only timestamp recovery above is
   the sole deterministic exception; it never broadens schema/type validation.
4. **Publish immutable artifact.** Store the result, full version identity, dependency identities,
   context coverage, chronological frontier status, generation time, and usage-receipt ID. Never
   overwrite an artifact at a different content hash.
5. **Project.** Update a cheap logical-key index pointing to available artifacts. A projection may
   mix versions across time.
6. **Render.** Select the newest compatible artifact available for each logical key. Missing newer
   fields are absent/unknown in the UI, not fabricated and not a build failure.

Steps 1 through 5 use the shared runner contract for all six summarizers. Projection provenance and
the logical-key catalog are common. Readers accept registered compatible prompt versions; an
agent-name artifact that lacks the later lifetime paragraph renders that field as unavailable.

## Context completeness and frontier metadata

Each optional context channel records a requested count, provided count, and unit. Examples are
`ancestor_transcript` in characters, `prior_days` in summaries, `prior_weeks` in summaries,
`source_occurrences` in occurrences, and `project_overview` in artifacts.

Raw counts with unlike units are never added together. Each channel is converted to a 0–100%
ratio, and the simple `coverage_percent` is the equal-weight average of those channel ratios;
`missing_percent` is its complement. Component-level values remain authoritative so a later policy
can make a better decision than the simple score. A job with no optional context channels is 100%.

Calendar artifacts also record one of:

- `project-start`: no earlier project period was expected;
- `contiguous-extension`: the immediately preceding same-level artifact was available;
- `isolated-backfill`: earlier project time exists but the contiguous predecessor was absent;
- `unknown-legacy`: an older artifact lacks enough provenance to decide.

This metadata permits, but does not itself trigger, policies such as “regenerate only when context
coverage improves from 50% to at least 80%.” Automatic resummarization remains opt-in because it
spends tokens.

## Version compatibility policy

- A prompt or model-visible instruction change increments the registered summarizer version and
  prompt-version string, with a changelog entry.
- An additive output field increments the output schema version. Readers must accept older schemas
  and represent absent fields as unavailable.
- A semantic change to an existing field requires a summarizer-version bump even when JSON shape
  is unchanged.
- Old immutable artifacts and receipts are retained. A targeted run may upgrade only one hour or
  day; neighboring older versions remain valid inputs when their fields satisfy the consuming
  summarizer's declared minimum schema.
- A build never silently treats an incompatible old artifact as current. It reports version and
  context coverage, then degrades to the best compatible artifact or normalized source data.
- Source mutation/truncation and a cache hash mismatch fail closed. Ordinary append-only source
  growth creates new staged inputs only for affected windows/frontiers.
- Provider-specific mutable auxiliary indexes are isolated from authoritative transcript prefixes.
  Orc conversation state may be rewritten only when its complete prior stable-spawn projection is
  proven to remain an unchanged subset. Task-note deletion is retained as a tombstone in the
  cumulative frozen projection; immutable overlap, new-ID high-water, and SQLite sequence guards
  still fail closed on mutation, reuse, or rollback.

## Required time/backfill behavior

The durable archive must support independent summary selection from ingestion range. In particular:

- Generate or backfill one exact hour, day, or arbitrary half-open interval without buying every
  later summary. `--summary-start-time`, `--summary-end-time`, and repeatable `--rollup-kind`
  control this independently from normalized ingestion.
- Record whether each calendar result extended a contiguous frontier or was isolated.
- Load sparse prior hour/day/week artifacts when available and record missing slots when absent.
- Sweep left-to-right later to improve context without making that sweep a prerequisite for a
  useful partial archive.
- Export one website slice into a separate package without truncating or changing durable
  normalized data or cached artifacts.

The proving target is all three teams from `2026-08-07T02:00:00Z` through
`2026-08-07T11:00:00Z` (22:00 Aug. 6 through 07:00 Aug. 7 EDT). The first paid proving run is only
the first hour and uses `gpt-5.5` at `medium` reasoning effort; provider failure must be reported
rather than silently falling back to another model or backend.
