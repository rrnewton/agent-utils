---
title: 'batch-seal-remove-retry: the post-end remove in test_real_host_batch_seal_waits_for_the_run_process has no pidns-teardown retry'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-09T10:52:50.000000000+00:00
updated_at: 2026-11-09T10:52:50.000000000+00:00
---

# Description

On 2026-11-09 a full gate (the #1086 validate-run-authority-budget (c)
commit 99e8c355) failed python.wrkslots.shard-1 in
py/wrkslots/tests/test_validation_run_liveness.py::test_real_host_batch_seal_waits_for_the_run_process.
After _end_run, the final `assert _remove_batch(host.project) == 0` saw
"RETAINED: slot01 reason=validation-run authority ... unknown use is not a free
slot". It passed 5 of 5 solo runs on the rebased commit.

Cause: the refused evidence was process evidence. The strict real /proc
snapshot (_absent_validate_process_snapshot, through
_processes_around_unit_enumeration in _run_evidence) refuses a partial population
while an unrelated process is mid-teardown (a PID-namespace init waiting in
zap_pid_ns_processes). The test already handles exactly that transient for
its live-phase remove with _refused_for_the_run (#389 gate-flakes), but the
post-end remove has no such wrapper. Under full-gate parallelism, one snapshot
hit the window.

# Design

- Give the post-end remove the same treatment as the live phase: retry only
  while the output names the pidns-teardown transient, and fail at once on any
  other refusal. Use a helper beside _refused_for_the_run that expects success
  (returncode 0) rather than a named refusal.
- Do not loosen the strict snapshot or the authority check itself (lead,
  2026-11-09).
- Check the other real-host tests in the file for unwrapped remove or finish
  calls after the run ends, and wrap them the same way.

# Acceptance Criteria

- The post-end remove retries only on the named pidns-teardown transient.
- Any other real-host remove or finish call found unwrapped is listed in Notes,
  wrapped or left with a reason.
- shard-1 passes in a full gate.

# Notes

Filed by the devbig030 coordinator at the lead's request, low priority, after
#1086 (c). Picked up by the wrkslots lane after #1086 (c) lands; it does not
block (c).
