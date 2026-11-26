---
title: 'vibe-talk-page-slice-cpu-budget: the app page suite runs in six slices so no slice reaches its 600 cpu-second budget on a loaded host'
status: open
priority: 1
issue_type: task
assignee: cl-flaketriage
labels:
- vibe-talk
- validation
- flake
created_at: 2026-11-26T18:30:00-05:00
updated_at: 2026-11-26T18:50:00-05:00
---

# Description

`vibe-talk.test.page-3` failed with `CPU-TIMEOUT >600s cpu` in 2 of the last 13 retained gates on 2026-11-26 and 5 more times in lane gate output, on heads whose changes it does not test. The app page suite (`tests/js/voice_page.test.mjs`) runs as four interleaved slices (`VOICE_PAGE_SHARD=i/4`), each with `cpu_timeout: 600`. In every retained gate the four slices used 440-605 CPU seconds at load, page-3 535-605: every slice runs at 75-100% of its budget, so ordinary suite growth or a loaded host tips one over. Most of a slice's CPU is the pinned esbuild service (390 of 605 CPU seconds in one killed run): the suite rebuilds the page bundle for each of its roughly 300 negative controls (`brokenScript` / `brokenModule`). The budget is a runaway guard, not a measurement, and a guard at the working set's edge is a flake (owner rule 346).

# Design

Test-only: run the suite in six slices instead of four. Add `vibe-talk/tests/dag.d/184-page-4.yaml` and `185-page-5.yaml` as copies of the slice step with `VOICE_PAGE_SHARD=4/6` and `5/6`, and change slices 0-3 to `i/6` with their descriptions saying six. The suite's own slicing (`fixture.mjs`, positions i, i+n, ...) and the graph contract test (every slice of one count is one step's, once) already handle any count. `cpu_timeout` stays 600 for every slice, so each now runs about two thirds of the old per-slice work: by the 2026-11-26 measurements about 380 CPU seconds at load, two thirds of the budget (five slices would leave about 480, too little margin on a truly hot host). Nothing else changes.

# Acceptance Criteria

- Six page slices `vibe-talk.test.page-0` .. `page-5` with `VOICE_PAGE_SHARD=i/6`, each with `cpu_timeout: 600`; the validation graph contract test passes.
- The six slices together run every test of the suite exactly once (the slicing rule), shown by one local run of all six whose test counts add up to the four-slice total.
- Measured CPU per slice on the next gates is recorded in the Notes; if a slice still exceeds about two thirds of its budget at load, that is reported here.

# Notes

Evidence from the gate journals (`step_end` `cpu_usage_usec`) of 11 retained gates on 2026-11-26 and the `process_snapshot` at the kill. Coordinator's choice (2026-11-26): more slices rather than a larger budget, six for two-thirds headroom (rule 346). Caching the negative controls' bundle builds would cut the suite's CPU far more, but changes test code beyond this fix; it gets its own follow-up issue.
