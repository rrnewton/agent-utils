---
title: 'wrkslotsd-ensure-states-deadline-test-wall-clock: the one-deadline lock test asserts elapsed wall time'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-24T03:00:00-05:00
updated_at: 2026-11-24T03:00:00-05:00
---

# Description

`ensure_states::lock_wait_tests::one_deadline_bounds_every_lock_in_a_mutation`
(rs/wrkslotsd/src/ensure_states.rs) asserts `started.elapsed() < 550 ms`.
Under host load it fails although the deadline logic is correct (it
passes alone in about 0.36 s), which breaks the hot-machine rule
against wall-clock assertions in gating tests.

# Design

Prefer an injected clock, or assert on the deadline logic itself (for
example, that one shared deadline object is passed to every lock wait)
instead of elapsed wall time. If that is large, quarantine the test
with this issue first.

# Acceptance Criteria

- The test no longer asserts elapsed wall time, or is quarantined
  with this issue named.
- The one-deadline behaviour stays covered by a deterministic check.

# Notes

Filed by mu-census from a gate observation on the #2081 re-stack
(99d24b53): the test is on main and flaked in the full Rust suite
under load (210 passed, this one failed on the 550 ms bound).
Fix after #2081 lands, as its own small change.
