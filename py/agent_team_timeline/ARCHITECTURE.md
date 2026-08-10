# Agent Team Timeline Architecture

This document is the cost and compatibility contract for `agent-team-timeline`. It must be updated
whenever a model prompt, model-visible input, generated output field, cache identity, or summary
selection rule changes.

## Durable project and export layout

The durable Hermit archive is `~/agent_logs_archive/summary/hermit`. One archive may contain
multiple teams under `teams/<team-slug>/`; source snapshots remain ignored, while normalized data,
expensive summary artifacts, model-usage receipts, and deterministic presentation files can be
version controlled. The intended teams for the first combined archive are `codex-coord-030`,
`claude-coord-176`, and `orc-coord-014`.

The durable archive and a website export are distinct concepts:

- The archive accumulates normalized team data and every validated expensive artifact.
- A slice export is a deterministic view over selected teams and a half-open UTC interval. It may
  contain only 2026-08-06 22:00 through 2026-08-07 07:00 EDT even though the archive retains more.
- Rebuilding an export never invokes a model. Missing summaries degrade to normalized transcript
  and statistics rather than causing unrelated cached summaries to be discarded.

That degradation is presentation-only. Missing phase, agent-lifetime, project-overview, or calendar
summary data receives an in-memory fallback labeled `Summary unavailable`; the builder writes
ordinary website/Markdown presentation files but never writes the fallback under `summary_data/`
or registers it as a model artifact. Compatible phase/rollup artifacts and source-bounded lifetime
artifacts take precedence. Project overviews require their source-bound projection, and summaries
whose ranges extend beyond a sliced export are not reused. Thus patchy archives preserve verified
paid work without leaking later knowledge into an earlier slice. A later `summarize` run upgrades
missing regions normally.

`build` retains its single-team archive behavior. `export` accepts repeated `--team` values and
composes those independently cached teams into one self-contained site. The compositor renders
each team without model calls, namespaces thread, phase, edge, glossary, artifact, evidence, and
detail identities by team, and aligns every track to one shared UTC interval. It merges team
project/host identity, events, statistics, rollups, summaries, and artifact catalogs while keeping
the team label on every filterable record. Repeating the same export is byte-idempotent; a managed
file inventory removes only stale files from an earlier combined export. Calendar rollups retain
the timezone in which each team's cached summary was generated; hourly keys are UTC-stable. The
export timezone controls date-bound parsing and the shared display axis, not cache reinterpretation.

### Read-only query boundary

`agent-team-timeline query` reads the deterministic presentation projection rather than model-cache
internals. Its only inputs are `data/timeline.json`, referenced `data/details/*.json`, and referenced
rollup Markdown beneath the selected archive root. Path resolution fails closed on absolute or
escaping references. Querying has no write path and can never invoke a model.

Canonical references are `team:<slug>`, `agent:<team>::<source-id>`,
`phase:<team>::<phase-id>`, and `rollup:<team>::<kind>::<start-ms>`. The query loader strips the
multi-team compositor's `<team>::` namespace from source and phase IDs before constructing those
references. Consequently, a reference remains stable when the same team moves between an
individual archive and a combined export. List projections expose references and concise metadata;
`show` resolves relationships and optional phase transcripts; literal search scans lifetime,
phase, calendar, and condensed-transcript text. JSON responses use query schema version 1, JSONL
emits one record per line, and Markdown is presentation-only.

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

For tasks, the authoritative append-only prefix is exactly `(id, task_id, content, created_at)`.
Server synchronization IDs, note authors, current task owners, and titles may be filled or rewritten
later, so their first observed values are frozen per note in the immutable provenance projection.
Subsequent drift increments explicit rewrite/degradation metadata but does not silently rewrite old
events or invalidate summary caches. A new note captures the then-current title and owner. A
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
| `plain-language-rollup` | 3 | active | hour, day, week, month, or quarter | `summarize.py:build_summary_prompt` (`plain-language-rollup`) |

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

### Project overview

Staged by `pipeline.py:_project_overview_job`.

The input is at most 48,000 characters from early root user/assistant conversation before a frozen
knowledge cutoff. Source event IDs, bounds, and a context digest are retained. The output is either
an evidence-supported newcomer overview or an explicit `Insufficient evidence` result. It may not
emit links. The projection is `summary_data/project_overview.json`; its first valid evidence epoch
is intentionally frozen against ordinary append-only growth.

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

This is a separate paid job, not formatting applied to the technical result. It receives analogous
lower-level and up-to-ten prior same-level plain-language results, plus the project overview and
supported chronological glossary definitions. It must identify the project for a newcomer and
describe content before opaque work-management identifiers. It shares each rollup projection with
the technical result but has an independent input hash and usage receipt.

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

When a backend's final message fails validation, or the phase timestamp rule repairs it, the exact
raw final message is preserved at `_usage/backend_outputs/<receipt-id>.json`. That receipt-linked
audit record includes a SHA-256 content hash, each job's half-open bounds, and the index, timestamp,
and `dropped-out-of-range` action for every repaired bullet. It deliberately excludes the model
prompt and captured Codex CLI stdout/stderr. For a nonzero backend exit, the durable failure
receipt and propagated exception retain the exit code but no captured stream detail. Ordinary valid
responses need no duplicate raw-output record.

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
  proven to remain an unchanged subset; the manifest records the rewrite/degradation while content
  blocks and task notes retain strict prefix validation.

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
