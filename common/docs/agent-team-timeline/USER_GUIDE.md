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
- Thick curved edges are structural forks (parent-to-child spawns) and joins (terminal
  child-to-parent results), so both remain visible. Detailed intermediate message edges are hidden
  globally by default and appear for the selected agent or work phase; both detailed-message
  behaviors have toolbar toggles.
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
- **Project glossary** opens the all-time terminology catalog. Exact, known glossary names in
  rendered summaries become links such as `#glossary/term-name-digest`. The renderer creates links
  only from IDs present in `timeline.json` and rejects duplicate or malformed targets, so model
  output cannot create a hallucinated glossary destination. The catalog begins with one durable
  newcomer project overview; every term shows its model-backed definition separately from the
  quoted first-use evidence that constrained it.

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
  --model gpt-5.6-luna \
  --reasoning-effort xhigh \
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
  --backend codex --model gpt-5.6-luna --reasoning-effort xhigh \
  --service-tier priority
```

Each stable time window gets a content-addressed cache key over:

- its transcript input;
- the substantial ancestor/coordinator scroll-back window;
- only glossary terms available by that point in history;
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
48,000-character transcript into version-controlled `summary_data/`. A second batched pass sees
that overview plus up to six retained source occurrences for each deterministic glossary term.
Each term freezes that first evidence set and records when the term became eligible for summaries;
a later second occurrence therefore cannot rewrite or evict terms from an already completed day.
Both passes exclude the half-open archive end boundary. Mutation or truncation of retained evidence fails
before any model call, while ordinary events appended beyond the recorded cutoff reuse the exact
overview and definition cache keys.

Knowledge jobs must either write a concise evidence-supported result or explicitly return
`Insufficient evidence`; acronym expansions and relationships cannot be guessed. URLs, Markdown
links, images, and link targets are rejected from generated knowledge. Only supported definitions
enter plain-language rollup prompts, and only for periods at or after their availability timestamp.
The overview, definitions, source event IDs, evidence excerpts, immutable epoch/cutoff metadata,
model, prompt version, input hash, and generation time are persisted under `summary_data/`; all of
their backend receipts are included in the same exact token accounting.

Unchanged keys are never sent to the model again. Term candidates are capped by eligibility time,
not first mention, and command lines, paths, database filenames, and unquoted uppercase prose are
excluded before they can consume definition tokens. New later terminology does not invalidate
older windows. A changed live window creates a new cache record while the previous valuable record
remains on disk. Each batch is committed only after every response in that batch validates against
the strict JSON schema; a failed batch cannot corrupt existing cache data, while other validated
batches remain reusable.

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

Summary selection is independent from ingestion. To backfill one exact hour while retaining the
archive's complete normalized source, run:

```bash
agent-team-timeline summarize \
  --team example-team --output ./timelines/example-team \
  --summary-start-time 2026-08-07T02:00:00Z \
  --summary-end-time 2026-08-07T03:00:00Z \
  --rollup-kind hourly --model gpt-5.6-luna
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
     -> durable project overview + evidence-bounded glossary definitions
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

The terminology scan runs before phase summarization. It records terms in introduction order by ISO
week, keeps the source sentence as first-use evidence, and supplies the chronological subset to each
phase and technical-rollup call. After phase summarization, the evidence-bounded definition pass
adds explanations for newcomer rollups without changing those earlier cache identities. This
discourages agents from inventing opaque “phase 2 / wave 9 / option B” labels and carries the user's
original subsystem/workstream names across spawn boundaries.

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
    ├── source_snapshots/             # gitignored verbatim complete-line rollout copies
    │   └── 2026/08/04/rollout-....jsonl
    ├── raw/
    │   ├── team.json
    │   ├── site-identity.json        # projects, repositories, hosts, archive timezone
    │   ├── source-manifest.json      # versioned path/byte/hash/update provenance
    │   ├── source-snapshot.json
    │   └── messages/<thread-id>.json
    ├── summary_data/
    │   ├── artifacts.json             # logical-key/version/model/context catalog
    │   ├── cache/<content-hash>.json
    │   ├── name_cache/<content-hash>.json
    │   ├── agents/<thread-id>.json   # hindsight name, lifetime summary + provenance
    │   ├── phases/<phase-id>.json
    │   ├── project_overview.json     # immutable evidence epoch + generated overview
    │   ├── rollups/{hourly,daily,weekly,monthly,quarterly}/... # both audiences
    │   ├── github/pulls.json         # ETag-backed bounded PR title/hover metadata
    │   └── glossary.json             # frozen definitions, availability + provenance
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
Conversation state supplies agent spawn records. Task notes are joined to their recorded author or
owner and mapped to the latest matching agent incarnation at that time; this is the best available
attribution when the task database does not retain owner changes per note. Reused names
become separate incarnation IDs while the official name remains visible. Each nonempty task note is
shown once as a worker-to-coordinator message; notes are not labeled terminal results because they
can represent incremental progress.

Some Orc installations discover nested coordinator sessions without persisting a parent identifier.
The importer preserves nested lineage when that field exists and does not invent a parent when it is
absent. Agent-close timestamps are also not always persisted, so a lifetime ends at its
last attributed activity or at the next reuse of the same official name.

## Codex source semantics and limitations

Codex disk logs are append-only across context compaction: old user, assistant, and tool records are
not deleted when the in-memory context is compacted. The importer deliberately ignores compaction
summaries and uses the original records.

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
