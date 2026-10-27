---
title: 'watch-agents-scale: a large registry makes watch-agents --once take minutes and print nothing'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
- performance
created_at: 2026-10-27T05:30:32.634927403+00:00
updated_at: 2026-10-27T06:22:11.704626307+00:00
closed_at: 2026-10-27T06:22:11.704626107+00:00
---

# Description

[opus 5.5] Another harness coordinator ran `agentctl watch-agents --once --check-only` on a registry of about 60 records (mostly done or idle) in a Herdr workspace of about 100 panes; it was killed by a 300 s timeout with no output. A trace of one pass showed every call serial and repeated: `herdr workspace get` about 27 times, `pane list` about 14 times, `pane process-info` six times per pane, `pane read --source recent` up to 5.4 s each, and `agentcloudctl list` 4.2 s. On this host one pass over a 64-record registry (48 watched) took 38 s, making about 600 subprocess calls: 191 pane get, 122 pane read, 114 agent get, 68 process-info, 47 workspace get, 14 pane list, 48 git check-ignore. The watcher also read every screen through `ManagedAgents::read`, which takes the agent lifecycle lock (so it waits behind a send) and rewrites the agent output.json in the registry.

# Design

1. One workspace get and one pane list per pass, cached; process-info at most once per pane per pass. 2. Skip records the watcher does not judge (not running, paused) and handle idle ones cheaply: no recent-screen read unless the visible screen changed; an agent whose pane is gone costs one call. 3. Bounded concurrent pool for per-agent checks, a timeout on every herdr and agentcloudctl call. 4. A per-pass time budget; agents not checked in time are reported as unchecked (partial result, never nothing), with progress on stderr under --verbose. 5. agentcloudctl list only when the registry has running agentcloud records, at most once per pass. 6. The watcher reads screens without the lifecycle lock and without writing output.json.

# Acceptance Criteria

A benchmark test with a fake herdr of 100 panes and 60 records (mostly done) finishes a pass in a few seconds; measured before and after against a real 64-record registry (read-only, --check-only, temporary --state-dir); gate green; release agentctl built for deployment.
