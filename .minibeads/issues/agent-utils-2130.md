---
title: 'tick-hub-deadline-and-dependencies: deadline-aware gate scheduling and typed same-tick dependency edges'
status: open
priority: 2
issue_type: feature
labels:
- tick-hub
created_at: 2026-11-22T15:45:00-05:00
updated_at: 2026-11-22T15:45:00-05:00
---

# Description

tick-hub runs a tick's gates strictly in config order with no deadline awareness, and depends_on does not order execution. A consumer's ticks overrun their deadline, and reordering gates could silently break undeclared dependencies.

# Design

(a) A tick deadline: a gate starts only if its declared time bound fits in the remaining budget; otherwise it is skipped by name, in config order, with its fired-state saved so it runs next tick. (b) Typed same-tick dependency edges: a gate may declare that it must run after another gate in the same tick (and whether it needs that gate to succeed); the engine orders by these edges, refuses cycles at load, and a skip of a prerequisite skips its dependants by name. The config loader accepts the new keys (it rejects unknown keys today).

# Acceptance Criteria

- Tests: a tick with a tight deadline runs only gates that fit and records skips; a dependency edge orders execution and a skipped prerequisite skips its dependant; a cycle is refused at load; existing configs behave unchanged.

# Notes

Requested by a consumer's lead from its tick-deadline analysis; filed by the coordinator.
