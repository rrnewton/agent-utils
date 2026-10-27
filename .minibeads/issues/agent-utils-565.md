---
title: 'headless-coordinator: run the coordinator agent without a TUI, behind the agentctl inbox'
status: open
priority: 2
issue_type: epic
labels:
- agentctl
- chat
- headless
created_at: 2026-10-27T00:03:42.077173025+00:00
updated_at: 2026-10-27T00:03:42.077173025+00:00
---

# Description

[opus 5.5] Owner request 2026-10-26: design and stage an option to run a coordinator agent headless. A small per-harness wrapper (Claude first, then Codex and Muse) drives the harness through its structured protocol, takes input from the agentctl durable queue, and writes a typed JSONL event log. agentctl tail NAME follows that log read-only, so a headless agent can still have a Herdr tab. The chat bridge delivers to a headless coordinator by appending to its queue and receives replies as events or through agentctl chat reply, with no screen capture. Design note: ai_docs/transient/2026-10-26-headless-coordinator/README.md (milestone M0).

# Acceptance Criteria

M0: design note landed with protocol, wrapper interface, tail, bridge changes, ops, security, migration plan M1-M5, risks, open questions, related work. M1-M5 per the note: event log and tail; Claude wrapper; bridge integration; one shadow day; cutover of the coordinator to headless with a documented rollback.
