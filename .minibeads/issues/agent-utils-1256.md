---
title: 'wrkslots-removal-ref-inventory: every removal records the refs it found, so a removal can be audited later'
status: open
priority: 1
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-11T01:20:00+00:00
updated_at: 2026-11-11T01:20:00+00:00
---

# Description

[opus 5.5] After #1255 wrkslots-private-history-refusal, a consumer audited six slots removed earlier and found it can't prove nothing was lost. The archive records keep each checkout's HEAD and salvage disposition, but not which branches and tags existed before removal. So slot-local history that was never recorded is unknown, not proven absent.

# Design

- Before deleting anything, removal writes a ref inventory into the archive record: for the checkout and for every submodule checkout, each ref name (refs/heads, refs/tags, refs/stash, refs/remotes, notes) with its target object ID, plus HEAD.
- The inventory is bounded: large ref sets are written to a sidecar file in the slot's salvage directory, with a count and digest in the record, so the record stays readable.
- No network access and no new refusals: the inventory is evidence only.
- Rust parity: same fields.

# Acceptance Criteria

- An archive record after removal lists every ref, with its object ID, for the checkout and each submodule.
- Tests count refs on fixtures (branches, tags, stash, a submodule with local refs) and check the record and the sidecar.

# Notes

Requested by the consumer that reported #1255. Related: #1255 (private history now refuses or is bundled) and #1257 (declared submodules' local refs).
