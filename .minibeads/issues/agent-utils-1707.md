---
title: 'agentctl-subagents-load-flakes: subagents tests that fail under full-suite load must wait on conditions or stop gating'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- testing
created_at: 2026-11-16T12:25:04.000000000+00:00
updated_at: 2026-11-16T12:25:04.000000000+00:00
---

# Description

Under full-suite load on a busy host, these agentctl Rust tests fail while
passing alone, in code the failing change did not touch (reported by two
lanes on 2026-11-15/16):

- subagents::tests transient_recovery_resumes... and
  watch_unit_repair_off... (reported in #1683's handoff);
- under investigation by #1430's lane: subagents::tests::restore::
  watch_types_a_queued_prompt_and_reports_a_stale_one and
  client::tests::herdr_compatibility::
  herdr_093_malformed_argv_is_not_unknown_process_evidence (round 3 of an
  acceptance run). A paired A/B is deciding whether either is a real
  regression before it is classed here.

The repository rule is that a gating check passes on a hot host (above 75%
CPU) and never depends on wall-clock budgets.

# Design

For each test: find the wall-clock assumption (a sleep, a fixed timeout, a
"stale after N ms" threshold, a poll count) and replace it with a wait on
the condition it means, with a generous upper bound that only catches
hangs. Thresholds that are themselves the behaviour under test (for example
"stale") are driven through an injectable clock, not real time. A test
that cannot be made robust quickly is marked non-gating with a pointer to
this issue until it is fixed.

# Acceptance Criteria

- Each listed test passes in 10 out of 10 full-suite runs on a host above
  75% CPU, or is non-gating with a reason recorded here.
- No test in the set asserts on elapsed real time.

# Notes

Related: #1704 (memory caps on the dagrun deps checks, the same class of
problem), #1683, #1430.
