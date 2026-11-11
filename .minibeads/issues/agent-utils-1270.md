---
title: 'wrkslots-recovery-ref-inventory: ownerless and absent-row recoveries record the refs they found'
status: open
priority: 2
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-11T04:45:00+00:00
updated_at: 2026-11-11T04:45:00+00:00
---

# Description

[opus 5.5] #1256 wrkslots-removal-ref-inventory records a ref_inventory in each salvage receipt of an agent-slot removal (remove, remove-agent-batch, retire-pending). The other paths that delete slot Git state don't: recover-ownerless-agent-worktree, -clone and -cache, recover-absent-agent-row(s), and the frozen-release path. Their records keep only HEAD and the salvage disposition, so a later audit can't tell which refs existed.

# Design

- Reuse #1256's ref inventory, with the same shape (scope, head, count, sha256, inline refs up to 64, else a sidecar), in each receipt those paths write: for the checkout and for every submodule or module repository they salvage.
- Evidence only: no network, no new refusals.
- Rust parity for whichever of these paths the Rust edition writes.

# Acceptance Criteria

- Each of those records lists every ref with its object ID, for each checkout and submodule.
- Tests count refs on fixtures; none use time.

# Notes

Follow-up to #1256 wrkslots-removal-ref-inventory, drafted by the #1255/#1257 lane.
