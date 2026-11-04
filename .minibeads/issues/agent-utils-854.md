---
title: 'wrkviz-summarize-skip-idle: no model calls for empty days or idle teams, a smaller prior context, plans calibrated from receipts'
status: open
priority: 2
issue_type: feature
assignee: mu-validate
created_at: 2026-11-04T22:56:39.989265159+00:00
updated_at: 2026-11-04T22:56:39.989265159+00:00
---

# Description

A zero-spend cost re-check of `wrkviz summarize` on a large log archive (harness report "Cost re-check", 2026-11-04) found that the summarize pipeline pays for work that produces nothing, and that plans for new teams are priced high:

- wrkviz calls the model for every calendar day in the window, even days with zero phases: 862 of 965 paid daily rollups had no child summaries.
- Teams with no phases in the window still get every per-team rollup, name and overview job: 303 of 608 planned teams, 19 jobs each.
- The 16,000-character prior-context block is about two-thirds of a median phase's input.
- Plans for teams without receipts use a built-in per-job table that is 1.3-1.45x high for phases and 4-9x high for rollups.

With the Muse backend (#839) the dollar cost is about zero, but wall time and call count still matter.

# Design

- Skip model calls for empty days and for teams with no phases in the window. The cache and the rendered site show "no activity" without a model call.
- A smaller default `--context-chars`. Measure the quality impact on one real team-day (a scratch copy only) and record the comparison, including the zero-spend plan difference.
- Calibrate `--plan` estimates from project-wide receipts (observed cost and latency per job kind) when available, falling back to the built-in table.

# Acceptance Criteria

- Tests: an empty day makes no model call and renders correctly; an idle team makes no rollup, name or overview call; plan calibration uses receipts when present.
- A before/after `--plan` on the same scratch archive shows the reduction in jobs.
- The userguide documents the defaults; gate passes.

# Notes

From the coordinator's cost re-check (scripts and data on the coordinator host's scratch). Assigned to mu-validate after #843.
