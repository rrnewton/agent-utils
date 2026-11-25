---
title: 'gate-agentctl-shard0-oom-hides-failures: python.agentctl.shard-0 is often OOM-killed, so the gate reads partial and real failures go unseen'
status: open
priority: 1
issue_type: bug
labels:
- gate
- dagrun
- agentctl
- validation
created_at: 2026-11-25T14:40:00-05:00
updated_at: 2026-11-25T14:40:00-05:00
---

# Description

The Python agentctl test shards (`python.agentctl.shard-0` and `python.agentctl.shard-1`) are killed at a learned dagrun memory cap of about 2.5 GiB before they run a single test. The scheduler reports the kill as `NO_RESULT: OOM-KILLED (hit inner MemoryMax ...) at a learned memory cap (cap≈2.5 GiB, peak≈2.5 GiB); a learned-cap breach under contention is not a failure (#1763)`. The gate then records the step as not run. A gate with no other red step comes out `partial` with exit 0, so a change that touches agentctl can look acceptable while its Python agentctl suite never ran.

The suite needs more than the cap. A full run of shard 0 on 2026-11-25 peaked at 2957.7 MiB, under run_pytest_shard's own 4000 MiB cap and above the learned 2.5 GiB. So the cap isn't a contention accident: the step can't finish under it, and each OOM kill leaves the learned value where it is, because a breach is "not measured".

Evidence from the gate journals of the last four days (all six such gates ran on 2026-11-25): 6 gates ran the Python agentctl shards. In 5 of them, both shards were OOM-killed with `tests_completed 0` after 70 to 184 s of wall time. Two of those gates came out `partial` with no failed steps, and one of them was killed after 123 s with 0 tests. Only one gate ran shard 0 to the end, and it found 8 real failures:

- 6 cases of `test_queue_waiter_refuses_archived_directory_or_replaced_lock` spun to the 600 s per-test timeout. Since 2f703edc (2026-11-21), the queue lock wait polled with a non-blocking flock even when the caller had no deadline, so the blocking-call hook in the test never fired.
- `test_send_inspects_terminal_artifact_when_another_drain_consumed_its_id`: its `_enqueue` test double didn't accept the `deadline_end` keyword that 2f703edc added.
- `test_retire_previous_boot_missing_pane_stale_agent_archives`: the live-holder scan read the host's real `/proc`, and another same-uid process happened to carry the fake's short session id.

These failures were latent on main for four days because the shard almost never ran. The fixes ride the #2217 branch.

# Design

- An OOM kill at a learned cap that is BELOW the step's own declared or recorded need is not "contention": it's a cap that can't be met. When a step with no completed tests is killed at its learned cap, raise the learned cap (at least to the configured `rss_baseline_bytes`, or by a step factor) before the next run, instead of leaving it for a re-profile that never happens.
- A learned-cap OOM that leaves a selected test step with zero completed tests must not let the gate read as passing. Either retry the step once at its declared cap within the same gate, or count it as a failed step (or as a distinct `not-run` verdict that blocks landing), so a `partial` row never stands in for a green one on the components the change touched.
- Give the Python agentctl shards an explicit memory hint (`rss_baseline_bytes` of about 3.5 GiB, as the Rust agentctl step declares 3 GiB) so the learned cap starts from a real need.
- Report it: the gate summary and ledger row name each selected step that ran zero tests, so a coordinator reading a `partial` row sees which suites never ran.

# Acceptance Criteria

- Test: a step OOM-killed at a learned cap with zero completed tests has its learned cap raised for the next run (it doesn't stay at the value it was killed at).
- Test: a gate whose only non-green outcome is a learned-cap OOM with zero completed tests on a selected test step does not record a passing or plain `partial` verdict for that step; it is retried at the declared cap or reported as not run, and the ledger row lists it.
- The Python agentctl shards declare a memory hint at least as large as their measured peak, and both shards complete in an ordinary gate on this host class.

# Notes

Related: #1763 (learned step memory caps too tight under gate contention), #2217 (carries the fixes for the three latent failure causes above), 2f703edc (the deadline change whose test fallout went unseen).
