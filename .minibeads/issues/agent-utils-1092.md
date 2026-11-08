---
title: 'run-liveness-mntns-reprobe-bound: validation-run liveness tests still refuse on an indeterminate mount namespace under load'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-08T21:50:00.000000000+00:00
updated_at: 2026-11-08T21:50:00.000000000+00:00
---

# Description

Recurrence of #906 wrkslots-shard-mount-namespace (closed). In full gates under host load (load average about 100-150), `python.wrkslots.shard-6` fails in `py/wrkslots/tests/test_validation_run_liveness.py`:

- `test_real_host_completed_removal_waits_for_the_run_process`
- `test_real_host_finish_recovery_waits_for_the_run_process`

with

> REFUSED: validation-run authority is unverifiable for slot slot01: validation-run evidence is unverifiable: mount namespace is indeterminate for live PID N after 4 re-probes over 150 ms: its mount-namespace link stayed absent but the process was not pro[ven gone] ...

Seen in gates on 2026-11-05, 2026-11-06 and twice on 2026-11-08 (two different #996 gate-flake-census-churn-abort gates; neither change reaches this path: process_censuses=0). The same tests pass solo. So the bounded re-probe that #875 and #906 added (4 re-probes over 150 ms) is too short under load for a PID whose `/proc/PID/ns/mnt` link is absent while the process is still alive: either it is exiting (a zombie or mid-exit) or its /proc entry is not yet readable.

# Design

- Find out what state such a PID is in when the link is absent (zombie? exiting? a kernel thread?) and whether that state already proves it can't use the slot (a zombie holds no mount namespace or cwd).
- Prefer a state-based answer (for example, treating a zombie or exited task as not using the slot) over a longer time bound. If a bound is still needed, scale it with load or make it deadline-based within the caller's budget.
- Keep refusing for a truly indeterminate live process; never treat an unreadable live process as absent.
- Python and Rust editions alike, or record the parity gap.

# Acceptance Criteria

- A test reproduces an absent mount-namespace link for a live-but-exiting PID and shows the new classification.
- The two liveness tests pass in full gates under load (cite ledger runs).
- No live process with an unreadable namespace is ever classified as gone.

# Notes

Filed by the devbig030 coordinator from cl-sync's #996 gate evidence (2026-11-08). Related: #875 wrkslots-census-exiting-pid, #906 wrkslots-shard-mount-namespace.
