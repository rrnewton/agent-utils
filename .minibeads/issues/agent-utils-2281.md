---
title: 'vibe-talk-client-perf-benchmarks: load-invariant counts for the hot client triggers, gated'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
- performance
created_at: 2026-11-26T06:00:00-05:00
updated_at: 2026-11-26T20:00:00-05:00
---

# Description

Client benchmarking and profiling on load-invariant measures (CPU time, instruction counts, event/layout counts; never wall clock) for the hot triggers (keystroke in composer and search, live arrival, refresh, scroll), as gating budgets with ceilings plus a measurement report. Owner 381 part B, from the perf lane plan approved by the coordinator.

# Acceptance Criteria

- A measuring harness reports counts per trigger, windowed lists on and off, with the measuring SHA and host load recorded.
- A counts-only DAG step gates the busiest triggers under ceilings from the baseline; no gated assertion is a time.

# Notes

## Resolution

Landed on github/main as f8126f58 (harness), 39623e86 (ceilings + DAG step vibe-talk.check.client-perf-budget-browser) and fdd85fd9 (strict-typing fix), gate pass on f41cbb30. Every gated assertion is a pure count (class writes, childList mutations, layout reads, Layout/RecalcStyle counts); long-task count and CPU time are report-only. The gated test process takes about 7 CPU seconds locally; the step's wall time in-gate was 56s at load1=116.
