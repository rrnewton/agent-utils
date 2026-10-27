---
title: 'wrkviz-summarize-budget-plan: summarize never produces a partial tree silently'
status: open
priority: 1
issue_type: feature
labels:
- wrkviz
created_at: 2026-10-27T01:57:17.193960959+00:00
updated_at: 2026-10-27T01:57:17.193960959+00:00
---

# Description

[opus 5.5] The owner, 2026-10-26 18:34: "This capping makes our summaries partial. That sucks and we wasted tokens on partial summaries I think we need to redo when adding more leaves to the map reduce tree. Finish it."

A long summarize run was held to a spending cap by an external guard that killed the process: it stopped after the phase summaries, with no rollups, and the next run could not tell what was left or what it would cost.

Wanted:
1. Plan before spend: estimate the whole map-reduce tree (leaves still to do, agent names, overview, every rollup level) before any model call, and refuse to start, with the estimate, when it exceeds the budget, instead of stopping midway.
2. Incremental rollups: when leaves are added, recompute only the rollups whose inputs changed (content-hash the inputs), and never redo paid leaves. Today a change to day N changes the prior same-level context, and so the identity, of every later day.
3. Retry a model answer under an invented key the way a left-out job is retried (a long run stopped on "unexpected key ..._dup_placeholder").
4. A budget stop mid-run leaves a clearly marked partial state that the next run resumes from.
Tests with a fake model.
