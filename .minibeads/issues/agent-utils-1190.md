---
title: "cross-path-filter: path-filter the gate-exclusive cross steps even under validate --all"
status: open
priority: 1
issue_type: feature
assignee: mu-gateprio
created_at: 2026-11-09T23:55:00.000000000-05:00
updated_at: 2026-11-09T23:55:00.000000000-05:00
---

# Description

Follow-on to #1180 gate-owner-priority Part B (harness
`ai_docs/2026-11-10-gate-exclusive-bottleneck.md`), approved by coordb on
2026-11-09. Measured on 2026-11-09 ET: the exclusive class held the host
7.83 h and accrued 23.9 h of wait; 26 of 27 top-level gates ran `--all`,
which forces all five gate-exclusive cross differentials
(`cross.agentctl.differential-0/1/2`, `cross.agentctl.herdr-agent-differential`,
`cross.herdr-run.differential`) onto every gate, while 14 of 28 gated heads
needed zero of them under the repository's own path selection and 6 more
needed only the four agentctl steps. Path-selected gates already skip
exclusive steps whose component no changed path selects
(`partition_gate_exclusive` in `scripts/validate.py`); `--all` opts back in
via `keep_all`.

# Design

- Under `--all`, compute the changed paths (existing `--base`, default
  `origin/main`) and the existing selection, and let
  `partition_gate_exclusive` keep only the exclusive steps whose component
  a changed path selects. The main phase still runs the entire portable
  contract; only the gate-exclusive steps are filtered.
- Fail-closed: an unclassified path (no prefix rule), a shared cross
  trigger (`cross/differential.py` or any path outside `cross/` whose
  groups include CROSS, per `_cross_shared_trigger`), or undeterminable
  changed paths keep all five steps.
- Plan output names the decision: which exclusive steps are kept, which
  are skipped, and why, in both `--list` and run output.
- `--cross-all` forces all five (release/nightly runs): with `--all` it
  restores today's behaviour exactly; without `--all` it keeps all five on
  top of path selection.
- No change to `--base` (path-selected) gates other than `--cross-all`.

# Acceptance Criteria

- `--all` on a page-only change (for example `vibe-talk/web/`) plans zero
  gate-exclusive steps and names all five as skipped.
- `--all` on an agentctl-only change keeps the four agentctl steps and
  skips `cross.herdr-run.differential` (and the mirror case).
- `--all` with an unclassified path or `cross/differential.py` changed
  keeps all five; `--all --cross-all` always keeps all five.
- Expected cross coverage follows the kept steps (a filtered-out step is
  not reported missing).
- Gated and landed like #1180 Part A.

# Notes

Filed by mu-gateprio for coordb (owner replies 286/288 via #1180). Estimated
saving on 2026-11-09 load: 3.7 h/day of host serialization. P1 batching and
P2 overlap re-measure after this plus phase-priority (#1146, enabled
2026-11-09) have a day of data.
