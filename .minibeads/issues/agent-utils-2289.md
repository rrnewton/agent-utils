---
title: 'gate-nonowner-atomic-slot-token: a cap on concurrent non-owner gates enforced atomically by a flock-held token'
status: open
priority: 1
issue_type: feature
labels:
- gate
- admission
created_at: 2026-11-26T07:48:00-05:00
updated_at: 2026-11-26T07:48:00-05:00
---

# Description

A host sometimes has to run fewer NON-owner gates at once than its total slot cap, for example while disk space is short, without slowing owner-priority gates. Today the only way to do that is from outside `scripts/gate.py`: a host-side keeper polls a condition and raises or drops an admission hold. That is not atomic. On 2026-11-26 such a keeper dropped its hold, and two non-owner gates were admitted within one poll window, against a line of one at a time.

gate.py already makes the total slot cap atomic: a grant re-reads `runs/` and writes the new run file inside one flock-held critical section. There is no equivalent for non-owner gates alone.

# Design

- A host setting `nonowner_slots = N` (N >= 1; unset means no separate non-owner cap) limits how many non-owner gates run at once. Owner-priority gates are not limited by it; they still count against the total slot cap as today.
- The cap is held by tokens: N token files in the admission directory, one per allowed non-owner slot. A non-owner ticket is admitted only if, inside the existing admission critical section, it takes one token with a non-blocking `flock`. It holds the token for the life of its run and releases it on exit. Because the kernel drops a flock when its holder dies, a crash or `kill -9` frees the token, with no reaping step. If no token is free, the ticket stays queued and does not block the admission lock.
- The token is held by the gate.py process that owns the run, not inherited by validation children (close-on-exec). That matches the existing rule that a run is reaped when its gate.py pid dies. The run file records which token it holds, so `gate status` and `gate queue` can show it.
- Where the setting lives: an older gate.py caps every waiter at 1 when `gate-slots.toml` holds a key it does not know (`host_slot_cap`). So a new key in that file would cap ALL gates, owner gates included, at 1 on hosts still running older checkouts, until they drain. Either use a sibling host file, as the defer, phase-priority and interleave settings do, or document that trade-off and accept it on purpose.
- Mixed versions: older waiters take no token, so they can still be admitted past the non-owner cap until they drain. The total slot cap is unaffected. Keep any host-side hold as a backstop until old waiters are gone.
- `gate status` shows the non-owner cap, the tokens held and by which run, and the oldest waiting non-owner ticket.

# Acceptance Criteria

- Deterministic test (no wall-clock waits or sleeps; synchronise on events, pipes or files): N deferred non-owner tickets are released together and race for admission; at most `nonowner_slots` run at once, and the rest stay queued.
- Test: a token holder killed with SIGKILL frees its token, and a waiting non-owner ticket is admitted next, with no reaping step.
- Test: with every non-owner token taken, an owner-priority ticket is still admitted up to the total slot cap.
- Test: with the setting unset, admission behaves exactly as before.
- The config location and the mixed-version behaviour are documented in gate.py's module docstring.

# Notes

Related: #1370 (admission hold), #1406 and #1521 (deferral and shared measurement holds), #2253 (gate shard OOM). Tests belong with the existing admission tests in `py/tests/test_gate_admission.py`.
