# agent-team-timeline

Build a self-contained, zoomable website from coordinator and subagent transcripts. The Codex
importer reconstructs the fork/join tree, agent lifetimes, tool/wait/idle intervals, interaction
edges, and full message views; cached model summaries add phrase, paragraph, daily, weekly,
monthly, and quarterly levels of detail.

The generated archive is ordinary JSON and Markdown plus dependency-free HTML/CSS/JavaScript. It
is designed to be committed, backed up, copied to another machine, and served with Python's built-in
web server.

```bash
agent-team-timeline refresh \
  --root-session SESSION_UUID \
  --team codex-hermit \
  --output ./agent-team-timeline/codex-hermit \
  --timezone America/New_York

cd ./agent-team-timeline/codex-hermit
make serve
```

Run `agent-team-timeline quickstart` for the short tour or
`agent-team-timeline --userguide` for the complete storage, privacy, and rerun contract.
