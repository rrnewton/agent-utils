# agent-team-timeline user guide

`agent-team-timeline` turns a noisy coordinator/subagent transcript into a durable hierarchy of
summaries and a zoomable local website. It answers both “what was happening at 02:13?” and
“what did this team accomplish this month?” without throwing away the underlying messages.

The importers support Codex multi-agent rollouts, Claude Code coordinator lineages, and Orc SQLite
coordinator/task history. The archive and browser schema is provider-neutral so additional adapters
do not require site changes.

## What the site shows

- Packed lanes are the default: non-overlapping agent lifetimes share the first available lane, so
  a long run does not grow one row per completed agent. Click a packed lane label to list every
  named agent assigned to that lane and select one directly. “Per-agent tracks” restores the full
  fork-tree view when that is more useful.
- Every agent has a hindsight short name based on its completed work, ancestor context, official
  coordinator path, role, and nickname. The short name is primary; hover, search, and detail views
  retain the full official path and coordinator nickname. The same hindsight pass writes a concise
  lifetime paragraph from all of the agent's work phases; hover shows that paragraph without another
  model call. Nested descendants are not depth-limited.
- A whole spawned interval is an **agent lifetime**; each summarized sub-block is a **work phase**.
  Work-phase boxes carry a short phrase at useful zoom levels. Their bottom strip distinguishes active,
  tool-running, waiting, idle, and explicitly blocked time.
- Thick curved edges are structural forks (parent-to-child spawns), joins (terminal
  child-to-parent results), and explicitly configured coordinator-session continuations, so all
  remain visible. A continuation does not invent a return arrow for the successor coordinator.
  Detailed intermediate message edges are hidden globally by default and appear for the selected
  agent or work phase; both detailed-message behaviors have toolbar toggles.
- Hovering a phase or edge shows its paragraph summary and statistics.
- Single-clicking selects an agent; a later single click on the same work phase narrows selection to
  that phase. Clicking a different phase selects that phase directly; clicking the selected phase
  again returns to the whole-agent selection. Click empty track background (or press Escape) to
  clear selection. Double-click opens three views: the cultivated Agent Work Summary, the full
  prompt/response transcript with tool use condensed to one line and role filters, and its rendered
  Markdown summary.
- Single-clicking a day, week, month, or quarter selects it; double-click opens rendered Markdown
  with **Technical** and **Plain Language** tabs. The latter introduces the project and explains
  specialized terms for a newcomer. Right-clicking a rollup, agent lifetime, or work phase offers
  range-appropriate zoom-to-fit actions that trim empty leading and trailing time without leaving
  the selected range. Horizontal trackpad gestures pan the time axis.
- The fixed footer recomputes user prompts, agent responses, inter-agent messages, tool calls, and
  active agents for the visible time range.
- Daily, weekly, monthly, and quarterly markers link the visible range to long-term summaries.
- Explicit GitHub pull-request URLs and `owner/repository#number` references become safe links in
  work summaries and transcripts. Ambiguous naked `#number` text stays plain unless importer input
  supplies repository context for that exact message.
- **Project glossary** is reserved for the all-time semantic concept catalog. The renderer accepts
  only supported, evidence-backed semantic concepts and never accepts model-authored link targets.
  During the semantic migration, definition-only records remain in the immutable cache for
  provenance and cost accounting, but they are not publication authority: a zero-token build leaves
  the catalog empty rather than link unclassified strings.

The browser is self-contained SVG/HTML/CSS/JavaScript. A pinned MIT-licensed `markdown-it` browser
bundle renders summary headings, lists, tables, blockquotes, and code with raw HTML disabled.
Markdown links, images, autolinks, and linkification are suppressed; after rendering, the UI adds
only schema-validated glossary, pull-request, and work-artifact links. It is vendored into every
archive, so the generated directory has no CDN or runtime package-manager dependency and remains
usable when copied offline.

## Install

```bash
python3 -m pip install agent-team-timeline
```

Python 3.10+ is required. Summarization uses an installed `codex` CLI by default; ingestion,
formatting, serving, and the deterministic `heuristic` backend use only the Python standard library.

## Find the coordinator session

Codex rollouts live under `~/.codex/sessions/YYYY/MM/DD/`. The first JSON line is `session_meta` and
contains `payload.id`, `payload.cwd`, and the source. A coordinator root has `source: "cli"`; child
metadata names `parent_thread_id`, `agent_path`, depth, and nickname.

For example, list recent roots for one checkout:

```bash
rg -l -m1 '"cwd":"/path/to/project"' ~/.codex/sessions/2026/08/05 \
  | while read -r file; do head -1 "$file"; done \
  | jq -r 'select(.payload.source == "cli") | [.payload.id,.payload.cwd] | @tsv'
```

Pass the coordinator `payload.id` to `--root-session`. Descendants are discovered by their stable
lineage `session_id`; no list of child files is needed.

If the same logical coordinator work restarted in a new root session, add its root UUID explicitly:

```bash
agent-team-timeline ingest \
  --sessions-root ~/.codex/sessions \
  --root-session ORIGINAL_ROOT_UUID \
  --continuation-session NEXT_ROOT_UUID \
  --continuation-session LATER_ROOT_UUID \
  --team example-team --output ./timelines/example-team
```

The option is repeatable in chronological order. Time proximity alone never links sessions. On the
first successful ingest, the manifest freezes the exact predecessor source line/timestamp,
successor start, and gap for each link. Later reruns may omit all continuation options and reuse the
recorded chain, or repeat its exact prefix and append another successor. Removing, reordering, or
replacing a recorded successor is rejected; use another archive if the chain was specified
incorrectly. `refresh` accepts the same option and applies it during its ingest stage before any
summary or build work.

Claude Code root sessions live under `~/.claude/projects/<encoded-project>/` as
`<session-uuid>.jsonl`. Pass that file to `--session-file`; the importer discovers nested
`<session-uuid>/subagents/agent-*.jsonl` files and their metadata sidecars recursively. It reads
assistant text, user prompts, tool calls/results, Agent spawns, and SendMessage edges while omitting
private thinking blocks.

For Orc, pass the project directory containing `.orc/` and `.tg/` as `--source-root`. The root
coordinator UUID names a directory under `.orc/sessions/`; child sessions with an explicit parent
identifier are followed as nested coordinators.

## End-to-end refresh

```bash
agent-team-timeline refresh \
  --sessions-root ~/.codex/sessions \
  --root-session 019fcfe7-0f68-7301-8aab-c2f90a7026c7 \
  --team example-team \
  --output ./timelines/example-team \
  --timezone America/New_York \
  --project example-project=https://github.com/example/example-project \
  --source-host build-host-01 \
  --backend codex \
  --model gpt-5.5 \
  --reasoning-effort medium \
  --service-tier priority \
  --summary-workers 3 \
  --name-batch-size 12
```

`refresh` is exactly `ingest`, then `summarize`, then `build`. It records a new immutable JSON run
receipt under `runs/` and updates `manifest.json` with the latest run and source digest.

The upper-left site title identifies what the archive contains. `--project LABEL=REPOSITORY_URL`
and `--source-host HOSTNAME` are repeatable; the first explicit project is the primary title link,
while secondary repositories and full hostnames remain available from the adjacent identity
disclosure. A combined dataset with no single primary uses the compact `multi-repo` label; more
than one execution host uses `multi-host`. Codex ingestion also reads `cwd`,
`git.repository_url`, and structured hostname fields
from session metadata. It never guesses identity from free-form prompt or response prose. Explicit
values and later structured discoveries are accumulated rather than discarding already recorded
projects or hosts. Orc backups commonly lack host metadata, so pass `--source-host` for them.

For Claude Code, use the provider-specific source selector; the remaining summary and build options
are identical:

```bash
agent-team-timeline refresh-claude \
  --session-file ~/.claude/projects/PROJECT/SESSION_UUID.jsonl \
  --team claude-project --output ./claude-project \
  --timezone America/New_York \
  --start-date 2026-07-31 --end-date 2026-08-03 \
  --backend heuristic --model deterministic-local
```

For one bounded Orc day, use its provider-specific source selector with the same summary and build
options:

```bash
agent-team-timeline refresh-orc \
  --source-root /path/to/project \
  --root-session SESSION_UUID \
  --team orc-project --output ./orc-project \
  --timezone America/New_York \
  --start-date 2026-07-21 --end-date 2026-07-22 \
  --backend heuristic --model deterministic-local
```

`--start-date` is inclusive and `--end-date` is exclusive in `--timezone`. Earlier transcript
context remains available to phase summarization, but the website, statistics, naming, and calendar
rollups are bounded to the requested dates. A daylight-saving boundary may therefore span 23 or 25
hours. Reuse the same bounds when refreshing an archive; use another output directory for different
bounds.

For a non-calendar slice, use RFC3339 instants with an explicit offset or `Z`:

```bash
agent-team-timeline ingest-orc \
  --source-root /path/to/project --root-session SESSION_UUID \
  --team orc-project --output ./orc-project \
  --timezone America/New_York \
  --start-time 2026-08-06T22:00:00-04:00 \
  --end-time 2026-08-07T07:00:00-04:00
```

`--start-time` is inclusive and `--end-time` is exclusive. A date and exact time may be mixed at
opposite bounds, but the date and time forms for the same bound are mutually exclusive. Instants
are canonicalized to UTC milliseconds in archive metadata.

### Start the website

Every output archive includes its own launcher:

```bash
cd ./timelines/example-team
make serve                 # http://127.0.0.1:8765/
make open                  # also ask Python to open the browser
make run-stats             # every pipeline run plus exact model-token accounting
PORT=9000 make serve
```

Or use the installed command:

```bash
agent-team-timeline serve --output ./timelines/example-team --port 8765 --open
```

Do not open `index.html` directly with `file://`; browsers block its JSON fetch. The launcher uses
Python's built-in loopback HTTP server and exposes nothing on the network.

## Separate stages

The separation is a hard cost-control boundary, not just an implementation detail.

### 1. Ingest — no model calls

```bash
agent-team-timeline ingest \
  --root-session SESSION_UUID --team example-team --output ./timelines/example-team
```

Ingestion follows the whole session lineage, removes forked-history duplicates, joins tool calls to
their completions, and writes canonical UTC timestamps. Verbatim user/assistant message text is
retained. Bulky tool stdout, command bodies, and patches stay in the authoritative Codex JSONL;
the archive stores their name, interval, status, and nested tool counts, which is enough for the
condensed transcript and statistics.

For an explicitly continued Codex history, pass ordered `--continuation-session` values as shown
above. Each successor must be a root session. The original session's normalized IDs and existing
summary cache inputs stay unchanged; common per-session event, turn, and tool IDs in successor
sessions are namespaced. The browser shows one structural continuation edge rather than pretending
that the new coordinator was a spawned worker.

Use `ingest-claude --session-file FILE` for a Claude lineage. Like Codex ingestion, it is offline and
does not invoke a model.

The Orc equivalent is `ingest-orc --source-root ROOT --root-session UUID`. It opens source
databases read-only, uses SQLite's online backup API for consistent copies, and parses only those
archive-local snapshots. Coordinator content blocks provide prompts, responses, and condensed tool
counts; agent blocks and task notes provide incarnation lifetimes and timestamped work updates.

### 2. Summarize — the only token-spending stage

```bash
agent-team-timeline summarize \
  --team example-team --output ./timelines/example-team \
  --backend codex --model gpt-5.5 --reasoning-effort medium \
  --service-tier priority
```

The CLI defaults to `gpt-5.5` at `medium` reasoning effort. A provider or model failure stops the
run and leaves its failure receipt; it never silently changes models or switches to the heuristic
backend. Use `--model` or the
`AGENT_TEAM_TIMELINE_MODEL` environment variable only when intentionally overriding that policy.

Each stable time window gets a content-addressed cache key over:

- its transcript input;
- the substantial ancestor/coordinator scroll-back window;
- supported semantic concepts available by that point in history (currently empty);
- model, reasoning effort, optional service tier, backend, prompt version, and summary schema.

Codex's catalog label **Fast** maps to the canonical service-tier value `priority`. Passing
`--service-tier priority` adds `-c service_tier="priority"` to every Codex summary and hindsight
naming invocation. Omitting the flag and passing `--service-tier default` are the same canonical
choice: every Codex child receives `-c service_tier="default"`, while its cache hash remains
compatible with summaries generated before tier support. Priority has a distinct summary and
hindsight-name cache identity. The effective value appears in immutable batch, invocation, and
top-level run receipts. A service tier is rejected with the deterministic heuristic backend.

Every calendar period has two distinct jobs and cache identities. The technical summary remains
content-led and must explain what a pull request, task, or phase changed instead of using its number
as an opaque referent. The plain-language summary is separately generated for a reader unfamiliar
with the project: it introduces the product from supplied evidence, explains specialized terms,
and treats work-management identifiers as supplementary evidence. Both jobs have independent batch
receipts and are included in the command's exact token accounting.

Before those calendar jobs, one separately cached knowledge pass reads up to 48,000 characters of
early root conversation and creates a durable newcomer project overview. Its first successful run
records an immutable source cutoff plus event IDs and a context digest, without copying the raw
48,000-character transcript into version-controlled `summary_data/`. It excludes the half-open
archive end boundary. Mutation or truncation of its retained evidence fails before any model call,
while ordinary events appended beyond the cutoff reuse the exact overview cache key.

The retired glossary-definition pass is disabled because deterministic candidate strings were too
broad to serve as a project ontology. New runs do not inject them into phase or rollup prompts,
do not launch definition calls, and do not rewrite `summary_data/glossary.json`. Old definition
caches, projections, and usage receipts remain on disk for audit and accounting. Builds ignore the
retired projection and publish no glossary links until an evidence-bounded semantic discovery pass
is implemented.

Unchanged keys are never sent to the model again. A changed live window creates a new cache record while the previous valuable record
remains on disk. Each batch is committed only after every response in that batch validates against
the strict JSON schema; a failed batch cannot corrupt existing cache data, while other validated
batches remain reusable.

Phase work bullets use half-open time bounds: the start timestamp is valid and the end timestamp is
not. A model response that is otherwise completely valid may contain out-of-range phase bullets;
the pipeline drops those bullets without shifting or inventing timestamps. It then derives the
phrase and paragraph only from remaining valid bullet text, or uses the existing transcript-only
fallback when every bullet was rejected. A genuinely empty model bullet list keeps the model's
prose. This exception is phase-only: malformed fields and out-of-range calendar-rollup bullets
still fail validation, and it does not alter the prompt version or cache key.

After phase summaries exist, a separate hindsight pass names every agent and writes its lifetime
paragraph in the same response. It sees the agent's
official path, coordinator nickname and role, arbitrary-depth parent path, cross-spawn ancestor
context, and the complete set of phase summaries describing what the agent ultimately did. This
means a reused or misleading coordinator name does not become the permanent UI label. Naming has a
separate content-addressed cache, so only an agent whose summarized work or context changed is sent
back to the model. Archives created before lifetime summaries incur one naming-only cache miss per
agent (batched normally); phase and calendar summaries remain cache hits. Later unchanged runs are
all-hit again. The whole summarize transaction holds the archive writer lock, preventing two
simultaneous refreshes from buying the same cache miss.

Every backend batch writes an immutable usage receipt with its model, reasoning effort, service
tier, input, cached-input, cache-write-input, output, reasoning-output, and total token counts.
Cache records link to that receipt. The command prints both tokens newly spent by this run and the
deduplicated original generation cost of all returned artifacts; an all-hit rerun therefore
reports zero new tokens without losing the original cost. Older cache entries remain valid and are
reported explicitly as having unknown original usage rather than being regenerated or counted as zero.
Failed backend final messages and timestamp-repaired phase responses are retained beside those
receipts under `_usage/backend_outputs/<receipt-id>.json`, with a SHA-256 hash, job bounds, and each
rejected bullet's index, timestamp, and action. These audit records store neither the prompt nor
captured Codex CLI stdout/stderr. A nonzero-exit receipt and displayed failure retain the safe exit
code but omit captured stream detail.

For compatibility, the cache reader accepts a paid artifact whose last bullet is exactly at its end
boundary. That artifact stays a cache hit and its bytes remain unchanged; newly generated
responses and deterministic fallbacks remain half-open.

Summary selection is independent from ingestion. To backfill one exact hour while retaining the
archive's complete normalized source, run:

```bash
agent-team-timeline summarize \
  --team example-team --output ./timelines/example-team \
  --summary-start-time 2026-08-07T02:00:00Z \
  --summary-end-time 2026-08-07T03:00:00Z \
  --rollup-kind hourly --model gpt-5.5 --reasoning-effort medium
```

Repeat `--rollup-kind` to request several levels. Omitting it retains the daily, weekly, monthly,
and quarterly defaults. Each generated artifact records whether it begins the project, extends a
contiguous same-level frontier, or is an isolated backfill, plus a component-level context score.
`summary_data/artifacts.json` retains every indexed model/version variant by logical key.
Aggregate accounting and receipt paths are also retained in the top-level `runs/*.json` metadata.
From the archive directory, `make run-stats` formats those top-level records as an oldest-to-newest
run history, including cache hits/misses, generated products, build counts, newly spent tokens, and
returned-artifact provenance costs. Backend receipts are attributed only when the team matches and
one top-level summarize/refresh command window contains their timestamps, so a failed run still
shows its completed and failed batches and their known token subtotal. Concurrent top-level
windows can overlap while waiting for the summary lock; ambiguous receipts remain unattributed
rather than being guessed. Receipt schemas and content hashes are validated before a record enters
the ledger. If even one attributed receipt lacks usage, the actual total is labeled `UNKNOWN`
rather than presenting that subtotal as a zero or complete cost.
The report separately scans the immutable backend receipt ledger and groups actual attempts by
backend, model, reasoning effort, and service tier. This archive-wide ledger includes a failed
attempt when the backend returned usage before failing; any usage-less receipt remains
explicitly unknown. Returned-artifact generation cost is never added to new spend.

For offline development or tests:

```bash
agent-team-timeline summarize \
  --team example-team --output ./timelines/example-team \
  --backend heuristic --model deterministic-local
```

The heuristic backend is intentionally less capable, but exercises the complete cache and rendering
pipeline without network or token use. It labels the overview and definitions as insufficient
evidence and retains first-use context instead of pretending to synthesize model-quality
explanations. Its cache keys are distinct from Codex summaries.

### 3. Build — guaranteed zero model calls

```bash
agent-team-timeline build --team example-team --output ./timelines/example-team
```

`build` reads normalized JSON and structured summary data, then regenerates every Markdown, detail
JSON, and website file. Identical bytes are not rewritten. Use it freely after CSS, layout, or
Markdown formatting changes. Because those presentation files live at the output root, the tool
refuses to use a non-empty directory unless it already contains its archive marker or a recognized
existing `teams/<slug>/raw/team.json`; a mistaken `--output` cannot replace another project's
README or Makefile.

Build fails closed on definition-only glossary projections: it ignores those retired records and
omits them from links and rendered catalogs. This removes stale glossary links and weekly generated
files without rewriting or deleting original definition caches, projections, artifact catalogs, or
usage receipts.

To create a separate zero-token website package for a selected interval, use `export` after the
needed summaries are cached:

```bash
agent-team-timeline export \
  --archive ./timelines/example-team --output ./exports/overnight-hour \
  --team example-team \
  --start-time 2026-08-07T02:00:00Z --end-time 2026-08-07T03:00:00Z \
  --rollup-kind hourly
```

The export has its own archive marker, run receipt, Makefile, and local server launcher. It reads
the durable archive but does not copy or truncate normalized source data and cannot invoke a model.
If a selected phase or rollup has no cached summary, the site labels it `Summary unavailable` and
still exposes its normalized transcript and statistics. These build-only placeholders never enter
the model cache; rerunning `summarize` later replaces them with paid summaries without discarding
already compatible cached work.

Repeat `--team` to combine providers or machines in one aligned site. Every team receives the
same half-open interval and rollup selection:

```bash
agent-team-timeline export \
  --archive ~/agent_logs_archive/summary/example-project \
  --output ./exports/example-project-overnight \
  --team codex-project --team claude-project --team orc-project \
  --start-time 2026-08-06T22:00:00-04:00 \
  --end-time 2026-08-07T07:00:00-04:00 \
  --rollup-kind hourly --timezone America/New_York
```

Combined exports namespace provider-owned identifiers and phase-detail paths by team, merge
project and host identity, and preserve team labels for filters, rollups, summary files, events,
and statistics. If archived teams disagree about their display timezone, pass one explicit
`--timezone` for date-bound parsing and the shared axis. Existing daily and higher rollups retain
each archive's calendar timezone rather than being relabeled as a different cached computation;
hourly rollup keys are UTC-stable.

### 4. Optional GitHub pull metadata — conditional and cached

```bash
agent-team-timeline github-metadata \
  --team example-team --output ./timelines/example-team
```

This scans only evidenced pull links, conditionally fetches their title, state, branches, author,
and a bounded body excerpt, then rebuilds the detail JSON. Public repositories need no token. For
private repositories, set `GH_TOKEN` or `GITHUB_TOKEN`, or name another environment variable with
`--github-token-env`; credentials are request-only and never enter the cache. ETags make unchanged
reruns byte-stable. `refresh --github-metadata` performs the same optional step after its normal
build. Successful records are saved individually, so a rate limit or unavailable repository cannot
discard metadata already fetched.

## Summary hierarchy

The pipeline performs real multilevel reduction:

```text
verbatim messages + condensed tools
     -> fixed, append-stable agent phases
     -> durable project overview
     -> daily technical + plain-language summaries
        -> weekly technical + plain-language summaries
           -> monthly technical + plain-language summaries
              -> quarterly technical + plain-language summaries
```

Each level keeps a phrase, a paragraph, and timestamped substantive work bullets. Calendar
boundaries are computed in `--timezone`, including daylight-saving transitions. UTC instants remain
canonical in JSON; the chosen IANA timezone controls labels and day/week membership. That choice is
stored in `raw/site-identity.json` with its provenance and is used by the browser independently of
the browser or web-server machine's local timezone. Archives without that file retain the
timezone in normalized team data; malformed or missing browser data falls back visibly to UTC,
never silently to ambient browser time.

The retired deterministic terminology scan remains as migration/test code only. It never supplies
new prompt context or creates model work. A future replacement must classify bounded chronological
evidence into projects, systems, subsystems, sustained workstreams, named tasks, and milestones,
and must establish each concept's availability before it can enter later prompts or links.

## On-disk format

An archive is ordinary text designed for version control:

```text
example-team/
├── .agent-team-timeline.json
├── index.html, timeline-core.js, app.js, style.css
├── vendor/markdown-it-15.0.0.min.js  # pinned offline Markdown renderer + license
├── Makefile, serve.py, run_stats.py, README.md
├── manifest.json
├── runs/<timestamp>-<hash>.json
├── data/
│   ├── timeline.json
│   └── details/<phase-id>.json
└── teams/example-team/
    ├── source_snapshots/             # gitignored validated source copies
    │   ├── 2026/08/04/rollout-....jsonl       # Codex/Claude
    │   ├── .objects/<prefix>/<sha>.db         # immutable Orc SQLite objects
    │   ├── .projections/<prefix>/<sha>.json   # frozen Orc note provenance
    │   └── .staging/                           # managed, retry-safe Orc candidates
    ├── raw/
    │   ├── team.json
    │   ├── site-identity.json        # projects, repositories, hosts, archive timezone
    │   ├── source-manifest.json      # versioned path/byte/hash/update provenance
    │   ├── normalized-generation.json # Orc manifest/team/catalog commit marker
    │   ├── source-snapshot.json
    │   └── messages/<thread-id>.json
    ├── summary_data/
    │   ├── artifacts.json             # logical-key/version/model/context catalog
    │   ├── cache/<content-hash>.json
    │   ├── cache/_usage/backend_outputs/<receipt-id>.json # failed/repaired raw output audit
    │   ├── name_cache/<content-hash>.json
    │   ├── agents/<thread-id>.json   # hindsight name, lifetime summary + provenance
    │   ├── phases/<phase-id>.json
    │   ├── project_overview.json     # immutable evidence epoch + generated overview
    │   ├── rollups/{hourly,daily,weekly,monthly,quarterly}/... # both audiences
    │   ├── github/pulls.json         # ETag-backed bounded PR title/hover metadata
    │   └── glossary.json             # optional retired definition projection; audit-only
    └── summaries/
        ├── agents/<thread-id>.md
        ├── phases/<phase-id>.md
        ├── hourly/YYYY-MM-DD/YYYY-MM-DDTHHZ-<team>-hourly.md
        ├── daily/<ISO-week>/YYYY-MM-DD-<team>-daily.md
        ├── daily/<ISO-week>/YYYY-MM-DD-<team>-daily-plain-language.md
        ├── weekly/<year>/YYYY-Www-<team>-weekly.md
        ├── monthly/<year>/YYYY-MM-<team>-monthly.md
        ├── quarterly/<year>/YYYY-Qn-<team>-quarterly.md
        ├── glossary/<year>/YYYY-Www-<team>-glossary.md
        └── glossary/<team>-glossary.md             # all-time catalog
```

`summary_data/cache/` is valuable generated data: keep it under version control if the archive is
versioned. It is what prevents a future refresh from buying the same summary twice. Run receipts are
append-only and make the last attempted/completed refresh, model work, cache hits, source size, and
source digest auditable.

Ingest first copies every newline-complete rollout in the selected lineage into
`teams/<team>/source_snapshots/`; all parsing after that point uses these copies rather than the live
Codex files. The generated root `.gitignore` excludes the potentially large copies, while
`raw/source-manifest.json` remains versionable and records each original path, copied byte count,
SHA-256 digest, line count, and last snapshot update. Reruns permit only exact reuse, an append to an
existing prefix, or a newly discovered child rollout. A disappeared file, shorter complete prefix,
or rewritten prefix fails the ingest before replacing any prior snapshot. An incomplete trailing
JSONL line is left for the next run.

For Codex histories with explicit successor roots, the same schema-1 manifest has an optional
ordered `continuation_sessions` array. Each entry retains the predecessor/successor IDs and source
paths, exact predecessor source line and UTC timestamp, successor start, and millisecond gap. Old
manifests without this field remain valid single-session archives. Once written, the evidence is
reused rather than recomputed as source files continue to append.

The snapshot, manifest, and normalized `team.json` update run under an archive-local Linux file
lock, so concurrent refreshes cannot publish an older validated copy over a newer one. Parsing is
restricted to the exact paths in that refresh's validated source set; leftover files from an
interrupted run are not silently adopted. Snapshot traversal and replacement reject symlinked
roots, directories, and targets, and durable replacements fsync both file content and its parent
directory before the transaction proceeds.

## Claude source semantics and limitations

Claude root and subagent JSONL files remain on disk across context compaction. The importer copies
their newline-complete prefixes plus immutable subagent metadata sidecars before parsing. Reruns
accept unchanged files, monotonic JSONL appends, and newly discovered descendants; disappearance,
truncation, prefix rewrites, or metadata changes fail without replacing the prior validated copy.

Subagent metadata supplies spawn depth, parent agent ID, role, description, and the spawning Agent
tool-use ID. That supports nested lineages and links the exact spawn prompt to its child. If metadata
is absent, the importer falls back to the root parent and the source agent ID rather than inventing
lineage. Turn boundaries are reconstructed heuristically from timestamped user messages because
Claude logs do not expose the same explicit turn lifecycle records as Codex. Text prompts in a
subagent log are counted as parent-to-child messages rather than human prompts, and a subagent's
final response is counted as a child-to-parent message while still driving its result edge.

## Orc source semantics and limitations

Orc's append-only content blocks are authoritative for coordinator conversation and tool execution.
Conversation state supplies stable agent spawn records. Local task notes are mapped to the matching
owner incarnation available at that time. Reused names become separate incarnation IDs while the
official name remains visible. A local note with no owner is preserved beneath an `Unattributed
Task Work` worker. A note carrying a server author is shown as an external/server-authored message
on the coordinator, counted separately from user prompts, and does not invent an agent or edge.
Each nonempty note appears once; notes are not labeled terminal results because they can represent
incremental progress.

The TaskGraph database set is reference-driven. A provider-initial session uses its named database,
or its session UUID when unnamed. Delegated sessions use their UUID even when a name was inherited;
`associated_dbs` adds any other referenced databases. References may precede file creation, so a
never-seen missing database is skipped. Once observed, a still-referenced disappearance fails
closed. A deliberately detached database remains frozen in the archive, and a replacement receives
a stable source ordinal so old event IDs and history remain intact. With an Orc session index, only
the selected subtree is inspected; without it, only the explicit root is inspected.

Orc may compact or rewrite its auxiliary conversation-state JSON even while content blocks keep
appending. The source manifest therefore treats content blocks and task notes as strict append-only
prefixes, but versions mutable projections separately. Task-note prefix identity excludes fields
that Orc may fill or change later: server synchronization ID, note author, current task owner, and
title. Their first observed per-note values are frozen in an immutable projection; later drift is
audited as a rewrite without silently changing old timeline events or paid-summary cache keys. A
new note captures the owner/title visible when it first appears.

A conversation rewrite is accepted only when every recorded AgentBlock spawn identity and value
remains an unchanged subset; missing or modified spawn evidence still fails before replacing any
prior snapshot. Schema-v2 manifests record bounded message/spawn digests, cumulative rewrite
counts, and explicit degraded flags—never the raw conversation payload. Existing schema-v1 archives
validate their preserved byte baseline and retain their exact raw-byte summary-cache identity until
a real semantic change moves them to deterministic schema-v2 identity.

SQLite backups are staged, integrity-checked, published into an immutable content-addressed object
store, and fsynced before the manifest is updated. `raw/normalized-generation.json` is written last
and binds the source manifest, normalized team, artifact catalog, and semantic source digest. A
reader rejects a stale/missing marker after an interrupted write; rerunning ingest repairs it and
then garbage-collects unreachable managed objects. Exact schema validation and component-wise
symlink rejection apply to both live log paths and archive paths.

Some Orc installations discover nested coordinator sessions without persisting a parent identifier.
The importer preserves nested lineage when that field exists and does not invent a parent when it is
absent. Agent-close timestamps are also not always persisted, so a lifetime ends at its last
attributed event/tool/turn/spawn or at the next reuse of the same official name. Child end times
propagate upward so parent lifetimes contain all recorded descendant activity; mutable Orc
`updated_at` values never extend a lifetime.

## Codex source semantics and limitations

Codex disk logs are append-only across context compaction: old user, assistant, and tool records are
not deleted when the in-memory context is compacted. The importer deliberately ignores compaction
summaries and uses the original records.

Separate coordinator root sessions are separate lineages unless the operator supplies ordered
`--continuation-session` values. This explicit-only rule avoids silently merging nearby but
unrelated work. A configured successor receives a collision-safe path beneath
`/root/continuation-<root-uuid>` and collision-safe normalized event/turn/tool IDs, while all IDs
and cache inputs from the original lineage stay unchanged. Its one continuation edge is structural;
it is neither a worker spawn nor evidence for a fabricated lifetime return.

Child rollout files begin with a copied parent-history prefix whose timestamps are rewritten to the
spawn time. The importer finds the child's first incoming task message and treats earlier records as
context only. Counting every raw line would otherwise duplicate much of the coordinator history and
create false activity spikes.

Codex persists spawn instructions, follow-up prompts, and mid-turn collaboration messages
as encrypted `gAAAA...` content in both sender and receiver rollouts. An offline exporter cannot
decrypt it. The archive preserves the availability state and ciphertext in normalized message data;
the UI clearly says the exact body is unavailable and shows an inferred paragraph from the receiving
agent's subsequent work. Automatic final answers and each agent's own commentary/final response are
plaintext; encrypted instruction bodies remain unavailable to the archive.

## Inspect and troubleshoot

Agents and shell scripts can navigate the same built archive without starting the website. JSON is
the default; `--format jsonl` streams one result per line, while `--format markdown` is convenient
for a terminal transcript.

```bash
agent-team-timeline query --output ./timelines/example-team list teams
agent-team-timeline query --output ./timelines/example-team list agents \
  --team example-team --start-time 2026-08-07T02:00:00Z \
  --end-time 2026-08-07T03:00:00Z
agent-team-timeline query --output ./timelines/example-team \
  show 'agent:example-team::SESSION_OR_AGENT_ID'
agent-team-timeline query --output ./timelines/example-team \
  show 'phase:example-team::phase-0123456789abcdef' --transcript
agent-team-timeline query --output ./timelines/example-team \
  search 'reproducible build' --scope all --team example-team --limit 20
```

`list` supports `teams`, `agents`, `phases`, and `rollups`. Its records carry canonical references
that `show` accepts directly:

- `team:TEAM`
- `agent:TEAM::SOURCE_AGENT_ID`
- `phase:TEAM::PHASE_ID`
- `rollup:TEAM::KIND::START_MS`

The references deliberately remove the compositor's presentation-only team prefix, so they remain
the same when an individual team is later included in a combined export. `show` adds parent,
children, and work-phase references for an agent; phase transcripts remain excluded unless
`--transcript` is explicit. Showing a rollup returns both its technical and plain-language Markdown.
Search is literal and case-insensitive by default. It can scan `summaries`, `transcripts`, or `all`;
`--agent` restricts work-phase and transcript results to one canonical agent reference. Time bounds
are half-open RFC3339 instants and select records whose intervals overlap the requested range.
Queries only read `data/timeline.json`, `data/details/*.json`, and referenced summary Markdown; they
never invoke a model or alter the archive. A generated archive also exposes `make query`, with
`QUERY_ARGS` defaulting to `list teams`.

```bash
agent-team-timeline inspect --output ./timelines/example-team
```

This prints track/phase/edge/event/rollup counts and the current manifest.

Common errors:

- **Root not found:** verify the UUID and `--sessions-root`; pass the root coordinator, not a child.
- **Missing phase or rollup summary:** run `summarize` with the same `--phase-minutes` used by `build`.
- **Codex backend failure:** the command prints a concise stderr excerpt and leaves all existing
  cache files untouched. Retry; independently validated batches from the failed invocation remain
  cached, while the failed or cancelled batches are regenerated.
- **Website is blank under `file://`:** launch `make serve`.
- **Wrong day boundary:** choose the desired IANA `--timezone`; never pass a fixed abbreviation such
  as EDT because it cannot model winter or daylight-saving transitions.

## Security and privacy

Transcripts may contain proprietary text, paths, prompts, and encrypted collaboration payloads.
Treat the archive like the source logs. The bundled server binds to `127.0.0.1`, the browser uses no
external scripts, and transcript text is inserted with safe DOM text APIs rather than interpreted as
HTML. Review repository visibility before committing an archive.

## Supported inputs and deployment boundaries

- Codex provider: implemented.
- Claude Code provider: implemented, including nested subagents and bounded local-date archives.
- Orc provider: implemented, including read-only SQLite snapshots, agent incarnations, nested
  coordinators when lineage is recorded, and bounded local-date archives.
- Multiple teams: the website schema and team filter are ready; one refresh currently writes one
  team archive. Merging team indexes is a follow-up.
- Additional adapters such as Gas Town should emit the same provider-neutral
  agents/turns/events/tools/edges model.
- Hosted serving: deliberately deferred. The local archive is complete and portable first.
