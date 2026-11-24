---
title: 'wrkslots-recover-absent-check-before-locks: recover-absent-validate-rows holds every lock for minutes just to refuse'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-22T03:50:00-05:00
updated_at: 2026-11-22T03:50:00-05:00
---

# Description

`wrkslots recover-absent-validate-rows --apply` takes all four registry locks, including the state-root flock, then loads the whole event history (about 95,000 files on one consumer host) and only then runs a Git-registration check that refuses ("Git still registers absent validation checkout"). Each call held the locks for 2-3 minutes just to refuse; called once per row by a consumer's reaper, it blocked heartbeats, validation launches and disk snapshots for over an hour.

# Design

Run the cheap refusal checks (Git registration, and any other precondition that does not need the registry) BEFORE taking the locks, the way the process census already runs outside them, then re-validate under the lock before acting. Load only the event history the decision needs (the event index where possible).

# Acceptance Criteria

- A refusable row refuses without taking the state-root flock; an accepted row still re-validates under the lock; the lock hold time no longer scales with the full event history; deterministic tests.

## Implementation notes

The cheap validation refusal checks now run before the registry
locks. A pre-check reads the requested rows through the event index
(`_scoped_status_state` proves the ACTIVE snapshot is current); the
locked path still re-runs every check against the full replay, so no
refusal is weakened. A row the plan is guaranteed to refuse — wrong
identity, checksum mismatch, or Git still registering the checkout
— refuses before any registry lock is taken instead of after minutes
of history parsing under them. Rows absent from the snapshot, stale
indexes, and any other unproven case fall back unchanged. Covered by
`py/wrkslots/tests/test_recover_absent_precheck.py`.

# Notes

Filed by the coordinator from a consumer report; the owning lane refines this file as it works.

## Resolution

[watermelon-full-preview] Landed on github/main in two parts. The row-only refusal pre-check (record identity, checksum, Git registration, locality, storage) landed as 304f431b + ad5c4972 + 6e9e6e0b, proven from the event-index snapshot before any registry lock and re-run unchanged under the locks; a mixed-input regression keeps the 'already-recovered' outcome byte-identical to the locked path. The follow-up (holds via the scoped index derivation, plus interrupted-mutation journal and validate-batch seal presence via _refuse_partial_state itself, with the command's own resumable batch falling through) landed as cc67c386 after a green full gate (3404dcdd) and a verified-overlap replay. The full history read for passing rows moved outside the locks separately in #2125 (mu-lockread). No new persisted fields.
