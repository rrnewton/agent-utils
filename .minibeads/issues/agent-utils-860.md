---
title: 'dagrun-priority-hints: ready steps dispatch by priority when parallelism is short, without adding dependencies'
status: open
priority: 1
issue_type: feature
assignee: mu-validate
created_at: 2026-11-05T01:17:10.423978592+00:00
updated_at: 2026-11-05T01:17:10.423978592+00:00
---

# Description

The owner (2026-11-04, via the other team's lead): "we shouldn't introduce artificial sequential dependencies. We should have maximum parallelism in the graph. But a notion of priorities or orderings among nodes which exist in parallel would be a hint to the planner that if it doesn't have sufficient parallelism it should start the higher-priority ones first."

Use case: a consumer wants cheap static checks dispatched before long build and test steps under contention, combined with fail-fast.

# Design

- A per-step `priority` (integer; higher dispatches first among READY steps; default 0). Today's LPT order (longest estimated first) is the tie-break.
- No effect on dependencies, admission or correctness: it only changes which ready steps start first when slots or resources are short.
- The journal records the dispatch order and each step's priority.
- Both editions (Rust and Python) if dagrun still has both. Update the dagrun skill, userguide and help.

# Acceptance Criteria

- With N slots < ready steps, higher priority starts first; with enough slots everything starts at once; with all priorities at the default 0, today's order is kept exactly.
- Schema validation accepts `priority` and rejects non-integers.
- Gate passes.

# Notes

P1, owner request via the lead (2026-11-04 20:16 ET). Assigned to mu-validate after #859.
