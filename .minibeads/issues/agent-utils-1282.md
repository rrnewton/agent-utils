---
title: 'gate-interleave-version-deadlock: never interleave-select a ticket whose waiter predates the flag'
status: open
priority: 1
issue_type: bug
assignee: mu-gatefast
created_at: 2026-11-11T09:58:00+00:00
updated_at: 2026-11-11T09:58:00+00:00
---

# Description

[watermelon-full-preview] Deadlock, 2026-11-11 4:23-4:45 AM ET (coordb report).
Main's native interleave (#1219 gate-admission-fairness) flagged an old
waiter's ticket `interleave_selected`. That waiter ran a pre-#1219
gate.py: it did not know the flag, so it deferred to an owner ticket.
The owner ticket's new gate.py deferred to the flagged ticket. Each
waiter waited for the other; no gate was granted until coordb set the
host interleave config to `after_owners = 99` (effectively disabling
native interleave) at ~4:45 AM ET.

The flag is only an in-memory ordering mark — it is never written to
the ticket file — so a mixed-version queue can hold a ticket whose
waiter cannot see the reason it was moved ahead, while a new waiter
treats the same ticket as the selected next grant. Mixed gate.py
versions are normal on this host: every slot runs its own checkout's
gate.py, and queued waiters keep the code they started with.

# Design

- Record the waiter's gate.py version in the ticket at enqueue (a
  build/feature marker naming the newest ticket field the waiter
  understands, e.g. `gate_version` or an `interleave_known` capability
  flag).
- Never interleave-select a ticket whose recorded version predates
  the `interleave_selected` flag: a waiter that cannot honour the mark
  orders by its own rules and can defer to a ticket that defers back.
- A ticket with no recorded version (written before this change) is
  treated as predating the flag: not interleave-selectable.
- Remove the host workaround (`after_owners = 99` in
  ~/.local/etc/agent-utils/gate-interleave.toml) once the pre-#1219
  waiters have drained and this has landed.

# Acceptance Criteria

- A focused test: a queue with a pre-flag-version (or unversioned)
  oldest normal ticket and two trailing owner admissions does not
  interleave-select the old ticket; a current-version ticket still is.
- The version/capability is recorded on enqueue and shown by
  `gate.py queue`.
- Mixed-version admission still never exceeds the slot cap.

# Notes

Filed by mu-gatefast with the hermetic-test fix that the deadlock's
host workaround exposed: test_interleave_puts_oldest_non_owner_next_after_two_owners
read the host's gate-interleave.toml (after_owners = 99) and failed on
clean main; the gate tests now point every per-host gate config at
test-owned paths.
