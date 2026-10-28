---
title: 'multiplexer-agnostic-doc: what it takes to run the suite on tmux as well as herdr (design doc only)'
status: open
priority: 3
issue_type: task
labels:
- agentctl
created_at: 2026-10-28T15:13:15.966277461+00:00
updated_at: 2026-10-28T15:13:15.966277461+00:00
---

# Description

[opus 5.5] Owner 2026-10-28 11:06 ET: document every direct herdr dependency (agentctl backends incl. #514 tmux lifecycle; agent idle/working detection via herdr agent_status vs #574 transcript-aware harness adapters; herdr-run; pane reads/sends; chat bridge delivery; watch-agents; wrkviz; agent-teams), map herdr operations onto tmux equivalents (incl. control mode -CC), list blockers (idleness detection first) and a migration path. No implementation.
