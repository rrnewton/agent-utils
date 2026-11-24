---
title: 'agentctl-watch-idle-claude-subagent-footer-false-positive: an idle lane is flagged done-while-working for its finished-subagent footer'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- watch
assignee: mu-maskpaths
created_at: 2026-11-24T05:29:41-05:00
updated_at: 2026-11-24T05:29:41-05:00
---

# Description

`agentctl watch-agents` flags idle Claude Code lanes as "status done while the screen shows work running (2 agents)". The trigger is Claude Code's "<- N agents" footer, which an idle session keeps showing for finished subagents. A process-tree check of a flagged lane shows only the harness processes at an idle prompt: no subagent is running. The notice recurs hourly for every idle lane, and a standing false positive can hide a real stall behind the noise.

# Design

Treat the Claude Code subagent footer as a running signal only when the process tree or the transcript shows a live subagent, or when the footer text itself indicates running work. Otherwise an idle prompt with a finished-subagent footer is idle, not done-while-working. Check whether the phantom-background family (#2045 agentctl-muse-phantom-background-send, where a finished background send stays counted as running in Muse and the watcher already cross-checks the screen count against the harness process tree) covers this signal; link the two, and share the cross-check shape where the signals are the same kind.

# Acceptance Criteria

- An idle Claude Code lane whose footer shows finished subagents, with no live subagent in its process tree or transcript, raises no done-while-working notice.
- A lane with a genuinely running subagent (live in the process tree or transcript, or footer text indicating running work) is still flagged when its status says done.
- The fix is pinned by a test with the idle-footer screen and the no-live-subagent tree, and the related #2045 check is linked or shared as the design finds.

# Notes

[watermelon-full-preview] mu-maskpaths. Number allocated by the coordinator. Related: #2045 agentctl-muse-phantom-background-send (the Muse phantom-background sibling; this issue is the Claude Code footer), #1991 (the watcher's existing screen-count versus process-tree cross-check). No consumer or team names anywhere, as always.
