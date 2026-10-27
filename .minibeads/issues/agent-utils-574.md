---
title: 'harness-adapters: one module per harness for screen hacks, plus transcript-aware stall detection'
status: closed
priority: 2
issue_type: feature
labels:
- agentctl
- watch-agents
created_at: 2026-10-27T12:39:13.623884282+00:00
updated_at: 2026-10-27T15:24:08.436397713+00:00
closed_at: 2026-10-27T15:24:08.436397492+00:00
---

# Description

[opus 5.5] Owner (2026-10-27): collect the per-harness special cases agentctl carries (screen markers, queued-input blocks, interrupt and submit keys, approval prompts, timer rows, session ids, resume flags, transcript locations, misdetection workarounds) in one place per harness, keep the cross-harness logic separate, and say which are workarounds for upstream bugs. Stall detection should look past the TUI at the harness's own transcript: the last event and its kind, a tool call in flight (command, elapsed, whether it set a timeout), and whether the model is producing output. Addition (owner 05:32): an auto-interrupt policy for watch-agents, on by default, replacing the per-agent --auto-deliver opt-in. It interrupts and delivers only when input is queued, the agent has been stuck (no progress on screen, in /proc or in its transcript) on at least --auto-deliver-checks consecutive checks spanning --auto-deliver-after minutes, no approval prompt or operator draft shows, and no tool is progressing. It allows a per-agent opt-out, sends a one-line notice per interrupt, and uses a lower threshold for urgent messages.

# Acceptance Criteria

Inventory table in ai_docs/transient/2026-10-27-harness-adapters/README.md (cross-harness vs harness-specific, file and line, upstream-bug drafts). A HarnessAdapter trait with one module per harness (Claude Code, Codex, Muse, agentcloud), introduced behaviour-preserving with the differential tests green. Transcript readers for Claude, Codex and Muse found through the record's session id or the harness process's open files, with redacted fixtures. watch-agents reports transcript activity and feeds it into stream-silent, tool-blocked and a long-untimed-tool notice. The auto-interrupt policy, with its flags and tests. A release agentctl built from main and a READY file.
