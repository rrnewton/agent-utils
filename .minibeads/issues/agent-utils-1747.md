---
title: 'wrkslots-validate-batch-one-same-uid-census: one same-UID census for a fenced validate batch'
status: closed
priority: 1
issue_type: task
labels:
- wrkslots
created_at: 2026-11-16T18:16:44-05:00
updated_at: 2026-11-17T08:58:48-05:00
closed_at: 2026-11-17T08:58:48-05:00
---

# Description

[watermelon-full-preview] In `remove-validate-batch`, each fenced
slot gets its own same-UID census serially (2-8 inventories each),
although the census already takes several paths at once. The batch
pays that cost per slot under its shared deadline.

# Design

- Fence every slot of the batch first, then run one same-UID census
  over all fenced paths (the targets argument already takes several
  paths), as `remove-agent-batch` groups its checks.
- Do not skip the census; the privileged census still runs before
  the fence.

# Acceptance Criteria

- A stubbed-census test records one same-UID census covering every
  fenced path of a multi-slot batch, and per-slot use still refuses
  exactly the used slot.
- Single-slot and non-batch removals are unchanged.

# Notes

## Resolution

[watermelon-full-preview] Landed on main as
`c91d30bf96d2ee41c9819937d4384a1ad124e8d5` (gated tree `712aaf23`,
owner-priority gate pass; two independent reviews, FIX-FIRST items
addressed). A proof-free multi-slot `remove-validate-batch` now
fences every slot first, runs one grouped same-UID census over all
fenced paths, then completes each prepared finish, with a fresh
per-slot recheck before later deletions. Proof-carrying batches and
single-slot removals keep the serial flow. The seal's paired-finish
classification accepts one paired finish per distinct sealed target;
plain `wrkslots recover` finishes every paired finish and retires
the seal after the last, and create proceeds beside them. Tests
cover the grouped happy path, kill-after-prepare recovery in both
request orders, crash mid-prepare, duplicate pairing refusal through
the CLI, a user entering after the grouped census, and a later
slot's census failure reported while the batch continues.
