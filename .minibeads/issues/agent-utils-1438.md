---
title: 'proc-status-reclaim-recheck-race: wait on the exiting holder pidfd in the reclaim recheck tests'
status: closed
priority: 1
issue_type: bug
assignee: mu-hookdrift
labels:
- wrkslots
created_at: 2026-11-17T01:15:00+00:00
updated_at: 2026-11-17T01:15:00+00:00
---

# Description

`wrkslots/tests/test_proc_status.py::test_reclaim_recheck_catches_replacement_without_signaling_any_holder` closes the first child's stdin inside a `_read_process_stat` hook and relies on the child having exited within `_LiveUseRecheckBudget(0.05)`, so it races under load: the child may not notice EOF and exit inside 50 ms, the recheck then names the first holder instead of the replacement, and the test fails (seen as the lone red in a 64-step gate run, passing standalone). Two sibling tests share the pattern with the default budget: `test_reclaim_restarts_lsof_and_proc_after_exact_lsof_generation_exits` and `test_reclaim_restarts_proc_after_exact_proc_generation_exits`.

# Design

- Condition-based rewrite, not a quarantine marker: in each hook, after closing the child's stdin, wait until the child's pidfd signals exit before the checker proceeds. The wait observes the exit condition (pidfd readable), never a wall-clock verdict, and leaves the child unreaped so the checker's own pidfd and generation reads still observe the exact generation.
- Keep `_LiveUseRecheckBudget` itself unchanged as product behaviour; only the tests' synchronisation changes.
- Proof: 5 runs of the fixed tests under deliberate CPU load (one systemd scope capped at 8 CPUs, oversubscribed 4x by 32 burners), plus the same 5 runs of the old test from a pristine checkout, which should fail at least once under the same load.

# Acceptance Criteria

- The three tests wait on the holder's exit condition and pass 5 of 5 under the capped load scope.
- The old test fails at least once in 5 runs under the same load, proving the harness reproduces the race; if it never fails, that is reported instead of claiming the proof.
- No product code changes; the full gate passes on the delivering tree.

# Notes

Gate 4's lone red in the #1436 wrkslots-create-journal-hook-drift validation, `wrkslots/tests/test_release_frozen_rust.py::test_release_yes_matches_python`, is noted here and not fixed in this issue: the Rust /proc census hit an exiting process (PF_EXITING, /proc fd unreadable) after its shared 2000 ms exit budget was already spent, which is another wall-clock dependence. Proposed follow-up: give the census an exit wait on the observed pidfd generation (condition-based, like this issue) instead of a shared wall-clock exit budget, with a load proof of the same shape.

## Resolution

- The four tests sharing the pattern (the target plus `test_reclaim_restarts_lsof_and_proc_after_exact_lsof_generation_exits`, `test_reclaim_restarts_proc_after_exact_proc_generation_exits` and `test_reclaim_fallback_exit_restarts_the_original_proc_census`) now close the holder's stdin and wait on its pidfd for the exit condition, on the first generation read, before the recheck budget starts; the child is left unreaped so the checker's generation reads still observe the exact generation. `_LiveUseRecheckBudget` and all product code are unchanged. The target also records the observation order (`lsof`, `first-exit`, `lsof`, `lsof`, optional fifth `lsof`): the replacement's own recheck runs only while the shared deadline lasts, so the lsof count is 3 or 4 under load while the sequence up to the find stays exact.
- Weakening check: with the recheck temporarily disabled (a local edit, reverted, never committed), the target test still FAILS, both before and after the call-count tolerance, so it still proves the recheck catches the replacement.
- Load proof: inside one systemd scope capped at 8 CPUs (CPUQuota=800%) oversubscribed 4x by 32 burners, the fixed tests pass 5 of 5 runs; the old test from a pristine checkout FAILED on its first run under the identical load, so the harness reproduces the race. An earlier attempt also showed the exit wait must sit on the first stat read: on the second read it consumes the 50 ms budget before the budget's own pidfd wait runs.
