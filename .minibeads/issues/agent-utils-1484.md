---
title: 'wrkslots-stale-partial-salvage-receipt-blocks-retry: a partial salvage receipt from an interrupted removal permanently wedges every retry'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- salvage
created_at: 2026-11-13T00:00:00.000000000+00:00
updated_at: 2026-11-13T00:00:00.000000000+00:00
---

# Description

[watermelon-full-preview] After a first removal attempt fails mid-removal, it leaves a partial salvage under the slot's salvage dir. A second attempt refuses with `local salvage archive receipt disagrees with the current checkout: .../wrkslots-salvage/<host>/<slot>/<gen>/<checkout>/receipt.json`, with no recovery path short of a new salvage root. Dead slots of 20+ GiB each are parked on this refusal.

# Design

A receipt from an interrupted or failed removal must be recognized as partial (journal or receipt status) and either resumed or superseded: keep the old archive aside with a reason and write a fresh one, never a permanent wedge. If the checkout really changed since the receipt, salvage again rather than refuse. Check Rust parity if the Rust edition has the same path.

# Acceptance Criteria

- A retry after an interrupted removal whose partial receipt disagrees with the current checkout completes, preserving the old receipt aside with a recorded reason.
- If the checkout changed since the receipt, the retry salvages the current state rather than refusing.
- A regression test fails before the fix and passes after, with no wall-clock waits.
- Existing lifecycle and salvage suites still pass.

# Notes

Reported 2026-11-12 from a consuming project's dead slots; the second attempt ran a newer build than the first, so receipt payload drift across builds must also supersede rather than wedge. Related: #1367 wrkslots-salvage-thin-bundle-fast, #1483 wrkslots-fence-rename-breaks-linked-worktrees.

## Resolution
