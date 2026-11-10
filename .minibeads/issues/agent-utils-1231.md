---
title: 'fold-latency-deflake: the fold latency browser check fails under gate load and passes alone'
status: open
priority: 1
issue_type: bug
assignee: mu-foldlat
created_at: 2026-11-10T17:49:28+00:00
updated_at: 2026-11-10T17:49:28+00:00
---

# Description

[opus 5.5] `vibe-talk.check.fold-latency-browser` (vibe-talk/tests/fold_latency_browser.py) is the most frequent load flake in full gates. On one host's gate logs it failed in 20 full gates between 2026-11-06 and 2026-11-10 (about 129 logs record it passing), in at least five lanes, and every failure that was rerun alone passed. On 2026-11-10 alone it failed in five gates across three lanes, each on a change that did not touch the page code it measures.

Its budgets follow #866: a main-thread work budget of 28 ms per key press (`--cpu-ms`, about 1.5x the 19 ms solo median measured at 1280x800 on 2026-11-06), twice that for the second largest press, and a wall budget of 8x the work budget. The work time is corrected by a reference layout workload measured just before each press, but the gate's usual load contends for memory bandwidth and caches rather than CPU, and the correction does not cancel that. Failing runs took 13.9 to 17.9 s of wall time for the step against about 9 to 12 s of user CPU, which is consistent with contention rather than a code regression. The failure message in the gate log is truncated, so the measured per-press times are not recorded there.

# Design

- First measure: record every press's work time, wall time, reference workload time and the host's load (gate admission running count, loadavg, PSI memory and CPU pressure) for each run, solo and under a representative gate, and print them in the failure message so the next red names the mechanism.
- Then either make the check load-aware (scale or re-measure the reference under the same contention, retry a failing press a bounded number of times and require a majority under budget, or judge the median of several presses), or move it out of the shared load (an exclusive tail step, like other timing-sensitive checks), whichever keeps the check's power to catch a real regression. The #532 restyle assertion is not timing-based and must stay as strict as it is.
- Keep the owner's #866 rule that the budget bites at about 1.5x the solo median; the change is about what "solo" means under load, not loosening it.

# Acceptance Criteria

- A run under a representative full-gate load passes repeatedly (for example 10 of 10) on main, while a deliberate slowdown in the fold handler (a mutation adding work beyond the budget) still fails, both solo and under load.
- The failure message prints the measured per-press times and the load evidence.
- The step's place in the DAG (shared or exclusive) is stated in the issue Resolution with the reason.

# Notes

Lead request 2026-11-10 (P1). Related: #866 budget rule, #905 second-press budget, #532 restyle scope.
