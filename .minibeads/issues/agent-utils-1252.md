---
title: 'timeline-benchmark-optin: timeline redraw budgets leave gating, and the functional fixture stops using a fixed port'
status: open
priority: 1
issue_type: task
assignee: mu-testaudit
labels:
- wrkviz
- test-robustness
created_at: 2026-11-10T22:46:59+00:00
updated_at: 2026-11-10T22:46:59+00:00
---

# Description

[muse watermelon] Owner, reply 346 (2026-11-10): "Disable those tests and have an agent work to bring them back when they're robust. Each product lane must make its tests fast, high signal, and robust to running on a hot machine (>75% cpu on 316 HW threads). ... Anything else should be disabled until we can improve it."

`timeline.browser.benchmark` enforces pan and zoom redraw budgets and `timeline.browser.benchmark-instrument` proves the benchmark records its metrics (`validation/timeline.dag.yaml`). A redraw budget is a timing measurement: on a hot host it measures contention, not the viewer. The benchmark already holds every browser slot (`browser: 8`) to measure in isolation, which serialises other browser checks behind a measurement nobody should gate on. Both steps leave default gating and run by explicit opt-in as a benchmark, in the pattern of #1248 fold-latency-behaviour-verdict and #1249 cross-differentials-robust.

`timeline.browser.functional` (10 gate failures in the last 3 days of the gate ledger, all 10 on heads that later passed a solo rerun) serves its fixture on a fixed port, so parallel gates collide; the three timeline steps serialise on `timeline-fixture: 1` for the same reason. Fix the fixture to bind an ephemeral port (bind port 0 and pass the chosen port through) if the change is small (under about 30 minutes); otherwise disable `functional` under this issue until the port fix lands. The functional interactions themselves are behaviour coverage and stay in gating once the port is ephemeral.

# Design

- Benchmark steps: same disabled-label filter as #1250 vibe-talk-timing-robust (excluded from path-selected and `--all` runs, named as skipped in plan output, restored unchanged by the opt-in).
- Functional fixture: bind port 0, read back the bound port, pass it to the browser base URL; remove the need for cross-gate port coordination. Keep `timeline-fixture` serialisation only if the shared output directory still requires it, and say so in the DAG comment.
- Return condition for budgets in gating: none planned. Budgets stay an opt-in measurement; a future behaviour-only redraw check (pan and zoom draw the right window) would be new coverage, filed separately.

# Acceptance Criteria

- `timeline.browser.benchmark` and `timeline.browser.benchmark-instrument` are absent from default `validate.py --list` output and present with the opt-in set.
- `timeline.browser.functional` passes with two instances running at once on one host (no address-in-use), or is disabled under this issue with the port fix scoped in Notes.
- Timeline behaviour coverage (functional interactions) still runs by default once the port is ephemeral.

# Notes

Filed by the test-robustness audit lane (mu-testaudit). Related: #1250 vibe-talk-timing-robust (same disable mechanism), #1249 cross-differentials-robust.
