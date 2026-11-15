---
title: 'hold-expiry: machine-checkable hold expiry and stale-hold surfacing'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] A hold is a hard protection with only a
prose reason (`{schema, machine, slot, held_at, reason}`): no
expiry, owner, or review condition. Transient rescue holds from
days earlier still blocked removal in the 2026-11-15 drain, with
nothing surfacing that their prose condition had cleared.

# Design

`hold --expires ISO8601` (and/or `--review-by`) recorded in the hold
event; status/audit surface expired holds as stale with their age
and reason. Expiry does not silently unhold: it makes the hold
reviewable by name, and the finisher reports it. A reaffirm command
extends a still-needed hold explicitly. Model flag HOLD_EXPIRY in
#1568 states the liveness contract.

# Acceptance Criteria

- Expired holds are listed as stale by status/audit/finisher.
- An unexpired hold still blocks remove, ttl, and clean-caches.
- Reaffirm/unhold remain the only ways a hold stops blocking.
