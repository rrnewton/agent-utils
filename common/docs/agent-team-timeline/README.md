# agent-team-timeline

Build a self-contained, zoomable website from Codex, Claude Code, or Orc coordinator and subagent
transcripts. Provider importers reconstruct the fork/join tree, agent lifetimes, tool/wait/idle
intervals, interaction edges, and full message views; cached model summaries add phrase,
paragraph, hourly, daily, weekly, monthly, and quarterly levels of detail.

The generated archive is ordinary JSON and Markdown plus dependency-free HTML/CSS/JavaScript. It
is designed to be committed, backed up, copied to another machine, and served with Python's built-in
web server.

## What it provides

- Real-time coordinator and nested-agent tracks with structural spawn/result fork-join edges and
  optional detailed intermediate messages.
- Active, tool-running, waiting, idle, and blocked intervals inside work phases.
- Hindsight short names and lifetime summaries grounded in completed work while retaining official
  paths and coordinator nicknames.
- Phrase, paragraph, cultivated work-summary, condensed-transcript, and rendered Markdown views.
- Separate technical and newcomer-oriented plain-language summaries for selected hours, days,
  weeks, months, and quarters, with an in-modal audience switch.
- A durable newcomer project overview and model-backed glossary definitions, each bounded by
  immutable retained source evidence, with append-stable availability and verified links from
  recognized terms.
- A linked site title grounded in structured repository metadata, with durable multi-project and
  multi-host identity plus explicit provenance.
- Versioned content-addressed model caches, a logical-key artifact catalog with context-quality
  scores, append-only raw-log snapshots, and immutable run receipts.
- A self-contained static website served by a built-in loopback server, with no CDN dependency.

Install from the package index:

```bash
python3 -m pip install agent-team-timeline
```

```bash
agent-team-timeline refresh \
  --root-session SESSION_UUID \
  --team example-team \
  --output ./timelines/example-team \
  --timezone America/New_York \
  --project example-project=https://github.com/example/example-project \
  --source-host build-host-01

cd ./timelines/example-team
make serve
# in another shell, inspect every run and the exact model-token ledger
make run-stats
```

Use `refresh-claude --session-file SESSION.jsonl` for a Claude lineage or
`refresh-orc --source-root PROJECT --root-session SESSION_UUID` for an Orc SQLite lineage. All
three refresh commands accept inclusive `--start-date` and exclusive `--end-date` local-calendar
bounds, or inclusive `--start-time` and exclusive `--end-time` RFC3339 instants for an exact
slice. Each bound may use either its date or time form. `--project LABEL=URL` and
`--source-host HOSTNAME` are repeatable; the first explicit
project is primary. Codex archives infer repository labels and links from structured session
metadata when no override is needed.

Every archive includes its launcher, static site, normalized message JSON, cached summary data,
rendered Markdown, source provenance, and run metadata. `make run-stats` prints per-run cache,
product, build, and token statistics, followed by the immutable backend receipt ledger. Receipts
are attributed to successful and failed summarize invocations; any usage-less receipt makes the
corresponding actual total explicitly `UNKNOWN` instead of zero. Repeating `summarize` on unchanged
input uses cached results; repeating `build` only regenerates deterministic presentation files. Use
`--backend heuristic --model deterministic-local` for an offline pipeline exercise.

The default summary configuration is `gpt-5.5` with `--reasoning-effort medium`. A provider or
model failure aborts the summary run and retains its failure receipt; the pipeline never silently
substitutes another model or the heuristic backend.

Summarization bounds are independent from ingestion. For example, backfill one exact hour without
truncating durable normalized data:

```bash
agent-team-timeline summarize --output ./timelines/example-team --team example-team \
  --summary-start-time 2026-08-07T02:00:00Z \
  --summary-end-time 2026-08-07T03:00:00Z \
  --rollup-kind hourly --model gpt-5.5 --reasoning-effort medium
```

After those cache entries exist, export the same zero-token website slice elsewhere with
`agent-team-timeline export --archive ./timelines/example-team --output ./hour-site --team
example-team --start-time 2026-08-07T02:00:00Z --end-time 2026-08-07T03:00:00Z --rollup-kind
hourly`. Repeat `--team` to align several independently summarized teams on the same real-time
axis; the generated site keeps team filters, identities, rollups, detail files, and artifact links
collision-safe.

Codex's catalog label **Fast** maps to `--service-tier priority`. Codex runs always override the
child CLI with an effective tier: omission and explicit `default` both mean `default`, while a tier
is rejected for the offline heuristic backend. The default tier keeps existing summary and
hindsight-name cache identities; priority receives distinct identities. Effective tiers are
retained in batch, invocation, and top-level run provenance.

Run `agent-team-timeline quickstart` for the short tour or
`agent-team-timeline --userguide` for the complete storage, privacy, and rerun contract.

The complete inventory of model-backed computations, their version histories, cache identities,
and compatibility rules is in [`ARCHITECTURE.md`](../../../py/agent_team_timeline/ARCHITECTURE.md).
