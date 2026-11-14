---
title: 'gate-admission-rerun-ticket-wall-clock: test_a_rerun_ticket_jumps_ordinary_queued_gates waits on a wall clock'
status: closed
priority: 1
issue_type: task
created_at: 2026-11-13T00:00:00+00:00
updated_at: 2026-11-14T22:40:19+00:00
closed_at: 2026-11-14T22:40:19+00:00
---

# Description

[watermelon-full-preview] `test_a_rerun_ticket_jumps_ordinary_queued_gates`
in `py/tests/test_gate_admission.py` failed once on a wall-clock deadline
with "tickets did not queue" under nice/idle-IO conditions; it passed on
rerun. Its `wait_queued` helper gives the admission queue only 10 s to
materialise each ticket, so a gate contending under load can flake the
test even though admission itself behaves correctly.

Owner rule 346: nothing real-time-sensitive gates. The helper already
polls on the actual queue condition (ticket files appearing); give that
condition a generous timeout instead of a 10 s wall clock, and do the
same for the scope-teardown poll in the same test.

## Resolution

[watermelon-full-preview] Closed 2026-11-14. Fixed in 5b0354bd: the rerun-ticket admission test waits on the queue condition (ticket files materialising) with a generous bound instead of the 10 s wall clock, per owner rule 346. The step passed in the subsequent full gates.
