---
title: 'watch-agents-stale-background-tasks: report screen-counted background tasks with no live process once, as info'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- watch-agents
created_at: 2026-11-20T15:30:00-05:00
updated_at: 2026-11-20T15:30:00-05:00
---

# Description

[watermelon-full-preview] A consumer lane reports: `agentctl
watch-agents` flags their Muse lane at every scan with "status idle
waiting on background work the screen counts (2 background shells),
but no shell runs under its harness". The lane checked: both tasks are
already terminal/completed and the work list is empty; the Muse
status area keeps showing them as running. That is stale harness
display, not live work, and the watcher already knows no shell runs.

# Design

- When the screen counts background tasks but no process runs under
  the harness (and the work list is empty), report it once as info
  (stale display), not "needs attention" at every scan.
- Re-flag only when the counted number or the process state changes.
- Test with a fake Muse screen.

# Acceptance Criteria

- A stale screen count with no live process produces one info line,
  then silence on unchanged rescans.
- A changed count or a live process re-flags.
- One commit, both editions where watch-agents exists.

# Notes

[watermelon-full-preview] Lane mu-panedead/1991, reserved by coordb,
queued after #1963 agentctl-redact.
