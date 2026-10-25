---
title: 'tmux-agent-lifecycle: run and manage agents entirely under tmux with no Herdr'
status: closed
priority: 2
issue_type: feature
created_at: 2026-10-25T17:17:41.953023265+00:00
updated_at: 2026-10-25T19:17:38.428058205+00:00
closed_at: 2026-10-25T19:17:38.428058094+00:00
---

# Description

Implement ManagedApi (start/adopt/stop/revive, process-identity proofs, readiness) for the tmux backend so agentctl can run and manage agents entirely under tmux with no Herdr, selected per profile/registry with Herdr staying default. Tmux harness detection beyond foreground command name via process tree under pane_pid so wrapped launches report status. tmux-started agents keep recording created_by/managed_by edges. Integration tests on a private tmux server (-L): start fake harness, send idle and deliver-while-working, read, stop, revive. Docs: userguide Running under tmux quickstart.
