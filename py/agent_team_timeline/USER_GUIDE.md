# agent-team-timeline user guide

`agent-team-timeline` turns a noisy coordinator/subagent transcript into a durable hierarchy of
summaries and a Perfetto-inspired local website. It answers both “what was happening at 02:13?” and
“what did this team accomplish this month?” without throwing away the underlying messages.

The first importer supports Codex multi-agent rollouts. The archive and browser schema are
provider-neutral so Claude, ORC, and Gas Town importers can be added without changing the site.

## What the site shows

- The coordinator is the first track; descendants follow in fork-tree order.
- Phase boxes carry a short phrase at useful zoom levels. Their bottom strip distinguishes active,
  tool-running, waiting, idle, and explicitly blocked time.
- Thick curved edges are spawns; smaller edges are later messages and returned results.
- Hovering a phase or edge shows its paragraph summary and statistics.
- Clicking a phase opens three views: the cultivated Agent Work Summary, the full prompt/response
  transcript with tool use condensed to one line, and its raw Markdown summary.
- The fixed footer recomputes user prompts, agent responses, inter-agent messages, tool calls, and
  active agents for the visible time range.
- Daily, weekly, monthly, and quarterly markers open the corresponding raw Markdown summary.

The browser is dependency-free SVG/HTML/CSS/JavaScript. The generated directory has no CDN or
package-manager dependency and remains usable when copied offline.

## Install

```bash
pip install "git+https://github.com/rrnewton/agent-utils#subdirectory=py"
```

Python 3.10+ is required. Summarization uses an installed `codex` CLI by default; ingestion,
formatting, serving, and the deterministic `heuristic` backend use only the Python standard library.

From a source checkout, run `./setup py`, then use `./bin/agent-team-timeline`.

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

## End-to-end refresh

```bash
agent-team-timeline refresh \
  --sessions-root ~/.codex/sessions \
  --root-session 019fcfe7-0f68-7301-8aab-c2f90a7026c7 \
  --team codex-hermit \
  --output ~/work/dev-hermit/agent-team-timeline/codex-hermit \
  --timezone America/New_York \
  --backend codex \
  --model gpt-5.6-sol \
  --summary-workers 3
```

`refresh` is exactly `ingest`, then `summarize`, then `build`. It records a new immutable JSON run
receipt under `runs/` and updates `manifest.json` with the latest run and source digest.

### Start the website

Every output archive includes its own launcher:

```bash
cd ~/work/dev-hermit/agent-team-timeline/codex-hermit
make serve                 # http://127.0.0.1:8765/
make open                  # also ask Python to open the browser
PORT=9000 make serve
```

Or use the installed command:

```bash
agent-team-timeline serve --output ./codex-hermit --port 8765 --open
```

Do not open `index.html` directly with `file://`; browsers block its JSON fetch. The launcher uses
Python's built-in loopback HTTP server and exposes nothing on the network.

## Separate stages

The separation is a hard cost-control boundary, not just an implementation detail.

### 1. Ingest — no model calls

```bash
agent-team-timeline ingest \
  --root-session SESSION_UUID --team codex-hermit --output ./codex-hermit
```

Ingestion follows the whole session lineage, removes forked-history duplicates, joins tool calls to
their completions, and writes canonical UTC timestamps. Verbatim user/assistant message text is
retained. Bulky tool stdout, command bodies, and patches stay in the authoritative Codex JSONL;
the archive stores their name, interval, status, and nested tool counts, which is enough for the
condensed transcript and statistics.

### 2. Summarize — the only token-spending stage

```bash
agent-team-timeline summarize \
  --team codex-hermit --output ./codex-hermit \
  --backend codex --model gpt-5.6-sol
```

Each stable time window gets a content-addressed cache key over:

- its transcript input;
- the substantial ancestor/coordinator scroll-back window;
- only glossary terms introduced by that point in history;
- model, backend, prompt version, and summary schema.

Unchanged keys are never sent to the model again. New later terminology does not invalidate older
windows. A changed live window creates a new cache record while the previous valuable record remains
on disk. Each batch is committed only after every response in that batch validates against the
strict JSON schema; a failed batch cannot corrupt existing cache data, while other validated
batches remain reusable.

For offline development or tests:

```bash
agent-team-timeline summarize \
  --team codex-hermit --output ./codex-hermit \
  --backend heuristic --model deterministic-local
```

The heuristic backend is intentionally less capable, but exercises the complete cache and rendering
pipeline without network or token use. Its cache keys are distinct from Codex summaries.

### 3. Build — guaranteed zero model calls

```bash
agent-team-timeline build --team codex-hermit --output ./codex-hermit
```

`build` reads normalized JSON and structured summary data, then regenerates every Markdown, detail
JSON, and website file. Identical bytes are not rewritten. Use it freely after CSS, layout, or
Markdown formatting changes. Because those presentation files live at the output root, the tool
refuses to use a non-empty directory unless it already contains its archive marker (or a recognized
legacy `teams/<slug>/raw/team.json`); a mistaken `--output` cannot replace another project's
README or Makefile.

## Summary hierarchy

The pipeline performs real multilevel reduction:

```text
verbatim messages + condensed tools
  -> fixed, append-stable agent phases
     -> daily summaries
        -> weekly summaries of daily summaries
           -> monthly summaries of weekly summaries
              -> quarterly summaries of monthly summaries
```

Each level keeps a phrase, a paragraph, and timestamped substantive work bullets. Calendar
boundaries are computed in `--timezone`, including daylight-saving transitions. UTC instants remain
canonical in JSON; the chosen IANA timezone controls labels and day/week membership.

The terminology scan runs before phase summarization. It records terms in introduction order by ISO
week, keeps the source sentence as evidence, and supplies the chronological subset to each model
call. This discourages agents from inventing opaque “phase 2 / wave 9 / option B” labels and carries
the user's original subsystem/workstream names across spawn boundaries.

## On-disk format

An archive is ordinary text designed for version control:

```text
codex-hermit/
├── .agent-team-timeline.json
├── index.html, app.js, style.css
├── Makefile, serve.py, README.md
├── manifest.json
├── runs/<timestamp>-<hash>.json
├── data/
│   ├── timeline.json
│   └── details/<phase-id>.json
└── teams/codex-hermit/
    ├── raw/
    │   ├── team.json
    │   ├── source-snapshot.json
    │   └── messages/<thread-id>.json
    ├── summary_data/
    │   ├── cache/<content-hash>.json
    │   ├── phases/<phase-id>.json
    │   ├── rollups/{daily,weekly,monthly,quarterly}/...
    │   └── glossary.json
    └── summaries/
        ├── agents/<thread-id>.md
        ├── phases/<phase-id>.md
        ├── daily/<ISO-week>/YYYY-MM-DD-<team>-daily.md
        ├── weekly/<year>/YYYY-Www-<team>-weekly.md
        ├── monthly/<year>/YYYY-MM-<team>-monthly.md
        ├── quarterly/<year>/YYYY-Qn-<team>-quarterly.md
        └── glossary/<year>/YYYY-Www-<team>-glossary.md
```

`summary_data/cache/` is valuable generated data: keep it under version control if the archive is
versioned. It is what prevents a future refresh from buying the same summary twice. Run receipts are
append-only and make the last attempted/completed refresh, model work, cache hits, source size, and
source digest auditable.

## Codex source semantics and limitations

Codex disk logs are append-only across context compaction: old user, assistant, and tool records are
not deleted when the in-memory context is compacted. The importer deliberately ignores compaction
summaries and uses the original records.

Child rollout files begin with a copied parent-history prefix whose timestamps are rewritten to the
spawn time. The importer finds the child's first incoming task message and treats earlier records as
context only. Counting every raw line would otherwise duplicate much of the coordinator history and
create false activity spikes.

Codex currently persists spawn instructions, follow-up prompts, and mid-turn collaboration messages
as encrypted `gAAAA...` content in both sender and receiver rollouts. An offline exporter cannot
decrypt it. The archive preserves the availability state and ciphertext in normalized message data;
the UI clearly says the exact body is unavailable and shows an inferred paragraph from the receiving
agent's subsequent work. Automatic final answers and each agent's own commentary/final response are
plaintext. Exact click-through for encrypted instructions requires a future runtime export hook that
captures plaintext before encryption.

## Inspect and troubleshoot

```bash
agent-team-timeline inspect --output ./codex-hermit
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

## Current scope

- Codex provider: implemented.
- Multiple teams: the website schema and team filter are ready; one refresh currently writes one
  team archive. Merging team indexes is a follow-up.
- Claude, ORC, Gas Town importers: planned; their adapters should emit the same provider-neutral
  agents/turns/events/tools/edges model.
- Hosted serving: deliberately deferred. The local archive is complete and portable first.
