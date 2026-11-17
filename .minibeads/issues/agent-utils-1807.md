---
title: 'wrkslots-remove-stale-active-revision: prepared removal writes ACTIVE from a stale whole-registry state'
status: open
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T17:42:37.000000000-05:00
updated_at: 2026-11-17T17:42:37.000000000-05:00
---

# Description

[watermelon-full-preview] Every agent `remove`, `retire-pending` and `remove-agent-batch` takes the lock-free prepared path; a single remove is wrapped as a one-item batch. `_prepare_retirement_removal` loads the registry state with no lock, then runs salvage lock-free, often for minutes, and captures the registry identity only after salvage. Under the mutation lock, `_assert_prepared_removal_current` compares that late identity and only this slot's record, so it passes even when another slot's heartbeat event landed in between. The remove then passes the stale whole-registry state as `previous=` to `_write_active_state`, which trusts a passed `previous`: its replayability check judges the new event against that same stale state, never against the log, and it writes ACTIVE from the stale state. Result: an active-state-recorded event with previous_revision N-1 -> N appended after another event already moved the revision to N; every later load refuses with "append-only active-record event revision does not continue the derived state for slot X", and ACTIVE silently loses the other slot's update (a heartbeat rolled back). This corrupted a live wrkslots event log and wedged every wrkslots command on that registry. No new persisted field (event payload, state record, journal) may be added: a pinned older reader must read everything this fix writes.

# Design

- Under the lock, immediately before any append, re-derive the active revision from the log. If it differs from the prepared state's revision, re-load the current state under the lock and re-validate this slot's removal against it, so the remove continues from the current state; defer with a clear retryable message only when this slot's own record changed. Prefer re-basing on the current state when this slot's record is unchanged: a remove must not fail just because another lane heartbeated during salvage.
- Defence in depth: whenever `_write_active_state` is given `previous=`, verify that `previous.revision` equals the revision the log currently ends at; refuse with a clear message if not and never append. ACTIVE must never be written from a stale state.
- Moving the identity capture earlier does not work: the retirement path appends an event lock-free after the state load, so every retirement would defer.
- Apply the same fix in both editions: Python `py/wrkslots/cli.py`, plus the Rust remove/retire writers in `rs/wrkslotsd` where they have the same prepared-state shape.

# Acceptance Criteria

- A deterministic regression test (no sleeps-as-sync) interleaves a removal's salvage phase, by hook or fake, with another slot's heartbeat/ttl-renew event, and proves: (a) appended events' revisions continue the log, (b) the log stays loadable by a fresh reader, and (c) ACTIVE equals the replay with the other slot's heartbeat preserved. Before the fix the test reproduces the exact refusal "append-only active-record event revision does not continue the derived state".
- A unit test pins the defence-in-depth check: `_write_active_state` given a stale `previous=` refuses and appends nothing.
- Both editions (Python and Rust) carry the fix and the tests where the prepared-state shape exists.
- No new persisted fields: no event payload, state record, or journal field is added, so the pinned older reader still reads the registry.

# Notes

[watermelon-full-preview] P0, owner-priority fast track. The diff goes to the coordinator before the gate.
