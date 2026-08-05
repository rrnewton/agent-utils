# Agent Team Timeline

`agent-team-timeline` turns long, noisy coordinator and subagent histories into a
Perfetto-inspired timeline plus a durable tree of Markdown and JSON summaries.
It is designed to answer both “what was this agent doing at 02:13?” and “what
did this team accomplish this week?” without discarding the original messages.

The current prototype imports Codex session logs. Its archive and browser data
model are provider-neutral so Claude, ORC, and Gas Town importers can follow.

## What it provides

- Real-time coordinator and nested-subagent tracks with spawn, message, and
  result edges.
- Active, tool-running, waiting, idle, and blocked intervals inside work
  phases.
- Hindsight short names derived from each agent's completed work while
  retaining official paths and coordinator nicknames.
- Phrase, paragraph, cultivated work-summary, condensed-transcript, and raw
  Markdown levels of detail.
- Daily, weekly, monthly, and quarterly summaries.
- Content-addressed model caches, append-only raw-log snapshots, and immutable
  run receipts for idempotent refreshes.
- A self-contained static website served by Python's built-in loopback server;
  no hosted service or CDN is required.

## Quick start

From this repository:

```bash
./setup py

./bin/agent-team-timeline refresh \
  --sessions-root ~/.codex/sessions \
  --root-session SESSION_UUID \
  --team my-team \
  --output ./my-team-timeline \
  --timezone America/New_York \
  --backend codex \
  --model gpt-5.6-sol

cd my-team-timeline
make open
```

Every generated archive includes `Makefile`, `serve.py`, the static site,
normalized message JSON, cached summary data, raw Markdown summaries, and run
metadata. Repeating `summarize` on unchanged input produces cache hits and no
model calls; repeating `build` only regenerates deterministic presentation
files.

For an offline pipeline exercise, use `--backend heuristic --model
deterministic-local`.

## Documentation

- [User guide](USER_GUIDE.md) — installation, finding Codex sessions, pipeline
  stages, archive format, idempotency, safety checks, and limitations.
- [Related work](RELATED_WORK.md) — comparison with AgentsView, Perfetto,
  Langfuse, Phoenix, `llm-wiki`, and other OSS alternatives.
- [Gemini research input](ai_docs/gemini_related_work_research.md) — the
  original external report retained for provenance.

## Status

This is an actively developed prototype. Codex ingestion, nested lineage,
multilevel summarization, static rendering, source snapshots, hindsight agent
naming, strict Python typing, and browser smoke tests are implemented. Provider
adapters beyond Codex and hosted deployment are not yet implemented.

Python code is checked repository-wide with strict mypy and no explicit
`typing.Any`:

```bash
make mypy
```
