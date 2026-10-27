---
title: 'headless-m1-event-log: typed JSONL event log and agentctl tail NAME'
status: closed
priority: 2
issue_type: feature
labels:
- agentctl
- headless
depends_on:
  agent-utils-565: parent-child
created_at: 2026-10-27T16:25:26.055083547+00:00
updated_at: 2026-10-27T17:15:11.391559613+00:00
closed_at: 2026-10-27T17:15:11.391559493+00:00
---

# Description

[opus 5.5] Milestone M1 of #565 headless-coordinator. A headless agent writes REGISTRY/NAME/events/<generation>.jsonl (v1 envelope: v, seq across generations, id, ts, agent, generation, session, turn, causation, type, data) plus index.json; seq continues across crashes (partial final line truncated, stale index overruled); retention 90 days / 2 GiB. agentctl tail NAME follows it read-only and lock-free: one line per event, --follow across generations, --last, --since, --types, --json, --at SEQ, gap lines for holes. Records beyond the event log (the chat bridge's append-only request journal and agentctl ledger, design section 12.1) are not in this slice.

# Acceptance Criteria

Writer and follower with seq continuity, crash repair and retention tested; tail renders, filters, follows and prints gaps; landed through the gate.
