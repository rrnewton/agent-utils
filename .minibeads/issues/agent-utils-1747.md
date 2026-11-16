---
title: 'wrkslots-validate-batch-one-same-uid-census: one same-UID census for a fenced validate batch'
status: open
priority: 1
issue_type: task
labels:
- wrkslots
created_at: 2026-11-16T18:16:44-05:00
updated_at: 2026-11-16T18:16:44-05:00
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
