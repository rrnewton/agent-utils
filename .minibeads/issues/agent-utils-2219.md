---
title: 'wrkslots-cpu-time-budgets: remove and cache-census work budgets count CPU time, with a generous wall backstop'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- test-robustness
- hot-machine
created_at: 2026-11-24T13:54:00-05:00
updated_at: 2026-11-24T13:54:00-05:00
---

# Description

Several wrkslots operations stop themselves on a fixed WALL-clock budget. On a loaded host (load 150 to 270 on 316 hardware threads), the budget expires while the process is waiting for CPU, not while it's doing too much work. The operation then refuses, and gates go red on code that didn't change:

- `remove` (agent batch): "this remove-agent-batch item used its 60-second budget for work before deletion at the /proc scan after the path fence; nothing in the slot was deleted". Seen on 2026-11-24 in `test_remove_stale_revision.py::test_remove_rebases_when_another_slot_heartbeats_during_salvage` and `test_finish_provisioned.py::test_ignored_only_discard_finishes_and_remove_deletes`. Both pass alone.
- Cache census: "fresh cache finalization exceeded the fixed wall allowance of 5 seconds", plus the other "fixed wall allowance" refusals in the census traversal, root validation and nested-Git judgment. Seen in `test_census_subject_progress.py::test_exact_final_sweep_cost_completes_without_raising_fixed_budget[1024-1025]`.

The owner's hot-machine rule (2026-11-21) says normal work must be designed to run under high load, with cost limits on load-invariant measures (CPU time, instructions). A wall budget that trips under load is the defect, in the product as well as in its tests: a real `remove` on a busy host refuses for the same reason.

# Design

- Each such budget measures the operation's own CPU time (process or thread CPU clock, including children it waits on where that applies), with the same nominal limit.
- A separate, generous wall-clock backstop (for example 10 times the CPU budget, and at least several minutes) catches a real hang. Its refusal names it as a hang backstop, distinct from the work budget.
- Refusal messages name which limit tripped (CPU work budget or wall backstop) and the measured values.
- Keep the Python and Rust editions behaviourally equal. Audit every "budget" and "fixed wall allowance" site in py/wrkslots and rs/wrkslotsd, and convert each one that bounds work, not waiting.
- Tests prove the CPU budget with a deterministic fake clock or an injected CPU-time source. No test depends on real wall time.

# Acceptance Criteria

- Test: with a fake CPU clock that stays under budget while a fake wall clock passes the old wall budget, remove and the census complete. With the fake CPU clock over budget, they refuse, naming the CPU budget.
- Test: a fake wall clock past the backstop refuses, naming the hang backstop.
- The three tests named above pass under a hot host in the gate, and an audit table of every converted site is in the resolution.

# Notes

Related: #1765 (load-sensitive gate tests, step 1 quarantine). Raised by a consumer team's lead on 2026-11-24, who pointed at the hot-machine rule.
