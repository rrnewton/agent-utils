---
title: 'test-cost-ranking: justify every regression test by cost'
status: open
priority: 0
issue_type: task
labels:
- validation
- testing
depends_on:
  agent-utils-21: parent-child
created_at: 2026-09-25T04:10:41.205691928+00:00
updated_at: 2026-09-25T11:39:54.964293653+00:00
---

# Description

[gpt-5.6-sol] Capture machine-readable timing for every executable gate and every `py/` pytest case, attribute other runner cases to measured gates, rank the suite by cost, and record a retain, optimize, relocate, or remove decision for expensive coverage.

# Acceptance Criteria

The report covers every executable gate and every `py/` pytest case with duration and ownership; Rust/libtest, JavaScript/browser, cross-language, package, example, and application cases without a common per-case timing schema are enumerated under measured owning gates and tracked for unified telemetry follow-up; expensive tests have an evidence-backed coverage rationale; fixed waits and redundant coverage are optimized; retained integration tests run only in relevant lanes.
