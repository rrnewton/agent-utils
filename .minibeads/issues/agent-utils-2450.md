---
title: 'gate-test-foreign-scope-leak: the gate admission test''s leaked-scope check compares every gate scope on the host before and after, so a real gate starting mid-test reads as a leak'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- gate
- tests
- hot-host
created_at: 2026-11-28T05:52:00-05:00
updated_at: 2026-11-28T05:52:00-05:00
---

# Description

`py/tests/test_gate_admission.py::test_a_rerun_ticket_jumps_ordinary_queued_gates` ends with a leak check. Before the test it lists every live `agent-utils-gate-*.scope` user unit on the host. After it, it lists them again and fails on any scope that is new.

Every gate on the host runs its command in a scope with that name pattern: `agent-utils-gate-<run id>`, and `agent-utils-gate-preflight-<pid>` for the preflight. So any real gate that starts while the test runs shows up as a "leak". The test itself starts its gates with CPU containment off, so it creates no scopes at all.

Seen on 2026-11-28 at 5:45 AM ET. The gate for R3 (B)(a), head 76a9fe4b, failed `python.repository-infrastructure.test` on this check alone:

> AssertionError: test leaked gate scope(s): ['agent-utils-gate-1795861381328074066-25de3953.scope']

That scope belonged to #2425's real gate (head a3cd4450), which started during the test. It will hit every gate that runs while another gate starts, and gates overlap routinely (3 slots on the lead host).

This breaks owner rule 346: a gating check must be high-signal and robust on a busy host. Other gates starting is normal load.

# Design

- The check counts only scopes the test's own gates could have made. Their run ids come from the test's private admission directory: history rows, plus any queue or runs ticket still there. The preflight scope names come from the test's gate process ids. The check never diffs the host's whole set.
- The existing generous poll for scope teardown stays. It waits on the condition, not a wall-clock budget.
- Negative controls, in-process, with a stubbed unit list (no real scopes, as the suite's fixture requires):
  - a foreign gate scope that appears mid-test must not fail the check;
  - a scope named for one of the test's own run ids, or for its gate's preflight process, must still fail it.
- Scan the other gate tests for the same pattern: a host-wide before/after unit, process or temp-dir census. Fix any found the same way.

# Acceptance Criteria

- The leak check names only scopes for the test's own run ids and preflight processes.
- Both negative controls pass: a foreign scope mid-test is ignored, and a leak of the test's own scope still fails.
- No other gate test diffs a host-wide set of units, processes or temp files that other runs can change. Any found is fixed the same way.
- The test passes in a gate while another gate starts during it.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (liaison 1159, 2026-11-28).

Evidence: gate 1795861230471579070-bf00db0c, step log `python.repository-infrastructure.test.log` (the assertion is at `tests/test_gate_admission.py:524`). The foreign scope's gate is 1795861381328074066-25de3953 in the same host ledger.
