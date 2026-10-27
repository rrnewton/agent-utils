---
title: 'headless-m2-claude-wrapper: persistent Claude Code stream-json wrapper as the queue''s only drainer'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
- headless
depends_on:
  agent-utils-565: parent-child
created_at: 2026-10-27T16:25:34.653895799+00:00
updated_at: 2026-10-27T16:25:34.653895799+00:00
---

# Description

[opus 5.5] Milestone M2 of #565 headless-coordinator. agentctl run-headless NAME runs one persistent claude -p stream-json process per agent (session resume when a transcript exists), is the only drainer of the agent's queue, settles each item on command_lifecycle started (queued stamps harness_queued_at; no queued within 30 s quarantines; cancelled before start returns it to its lane), delivers deliver:now / urgent items mid-turn and turn-boundary items between turns, recovers inflight items from Claude Code's transcript, restarts the harness with --resume (bounded), and serves a 0600 control socket (wake, status, interrupt, stop). start --mode headless --harness claude registers an adapter: harness-wrapper record; send, drain, status, list, stop, interrupt and compact handle it; install-units --headless NAME writes a systemd user unit; --permission-prompts none is the default and an example settings file with deny rules ships in the tree. Deferred: Codex and Muse wrappers (M2b/M2c, after the Claude cutover), agentctl say, takeover/release-headless commands, per-input cancel of a written input, watch-agents status from events, bridge integration (M3).

# Acceptance Criteria

Conformance tests against a scripted stream-json peer (settle on started, mid-turn now vs turn-boundary, interrupt, crash recovery, ack deadline, lock, tombstones); a real Claude Code run delivers, interrupts, compacts and resumes after restart; a test deployment replaces a low-stakes coordinator with rollback.
