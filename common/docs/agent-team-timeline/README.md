# agent-team-timeline

Build a self-contained, zoomable website from coordinator and subagent transcripts. The Codex
importer reconstructs the fork/join tree, agent lifetimes, tool/wait/idle intervals, interaction
edges, and full message views; cached model summaries add phrase, paragraph, daily, weekly,
monthly, and quarterly levels of detail.

The generated archive is ordinary JSON and Markdown plus dependency-free HTML/CSS/JavaScript. It
is designed to be committed, backed up, copied to another machine, and served with Python's built-in
web server.

## What it provides

- Real-time coordinator and nested-agent tracks with spawn, message, and result edges.
- Active, tool-running, waiting, idle, and blocked intervals inside work phases.
- Hindsight short names grounded in completed work while retaining official paths and coordinator
  nicknames.
- Phrase, paragraph, cultivated work-summary, condensed-transcript, and rendered Markdown views.
- Daily, weekly, monthly, and quarterly summaries.
- Content-addressed model caches, append-only raw-log snapshots, and immutable run receipts.
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
  --timezone America/New_York

cd ./timelines/example-team
make serve
```

Every archive includes its launcher, static site, normalized message JSON, cached summary data,
rendered Markdown, source provenance, and run metadata. Repeating `summarize` on unchanged input
uses cached results; repeating `build` only regenerates deterministic presentation files. Use
`--backend heuristic --model deterministic-local` for an offline pipeline exercise.

Run `agent-team-timeline quickstart` for the short tour or
`agent-team-timeline --userguide` for the complete storage, privacy, and rerun contract.
