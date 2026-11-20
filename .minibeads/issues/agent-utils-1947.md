---
title: 'retire-lock-scope-flaky-budget: retire lock-scope tests gated by the 60 s item budget on a hot host'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-20T00:00:00+00:00
updated_at: 2026-11-20T00:00:00+00:00
---

# Description

[watermelon-full-preview] `py/wrkslots/tests/test_retire_lock_scope.py::test_retire_pending_removes_without_candidate_validation_under_lock`
and `::test_retire_pending_defers_slot_changed_after_lock_free_prepare`
fail under load in gates (`assert [] == ['slot02']`, both slots retained)
yet pass alone. Alone, the two tests together take about 125 s: each
candidate's lock-free validation takes about a minute per slot on this
host, right at the 60 s `remove-agent-batch` item budget
(`_AGENT_REMOVE_BATCH_ITEM_SECONDS`). Under gate load the budget expires
first, the refusal is wrapped as `_PreparedRemovalRefused`, and the batch
retains slots the tests expected removed (or deferred by name). The
slot-change injection itself is deterministic: the mutated fingerprint is
computed before the late commit, so the locked re-check always sees the
new HEAD once it runs.

# Design

- Owner rule 346: no wall-clock budget gates a counted-evidence test.
  Lift the item budget in this file's tests to the 3600 s hang backstop
  other remove tests use (`#1195`), so the outcome is decided by the
  work finishing, not by a clock. The production 60 s guard and its
  scaling keep their dedicated tests in `test_lifecycle.py`; no
  production behaviour changes.

# Acceptance Criteria

- Both named tests pass on a host above 75% CPU, and the whole
  `test_retire_lock_scope.py` file passes with the same counted
  assertions (no validation under the flock, flock free between slots).

# Notes

Diagnosed by mu-flakes4 (taking over from mu-flakes3) from the probe
payload shape and a timed solo run; the throwaway probe file
`test_zz_probe_retire.py` was deleted as part of this fix.

## Resolution

Landed on github/main as `07b2b854` (mu-flakes4, 2026-11-20). Browser steps unverified on this host (libnspr4 missing) for #1936.
