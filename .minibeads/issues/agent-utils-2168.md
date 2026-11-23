---
title: 'wrkslots-declared-submodules-as-expected-nested: remove and retire should accept declared submodules as expected nested checkouts'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- lifecycle
created_at: 2026-11-23T14:00:00+00:00
updated_at: 2026-11-23T14:00:00+00:00
---

# Description

[watermelon-full-preview] Relayed 2026-11-23: slot reclaim hits remove and retire refusals on "undeclared nested checkouts" that are just git submodules inside a slot checkout (975 nested checkouts across 138 slots in one fleet; 922 detached at their gitlink, 10 on a branch). `adopt-checkout` cannot declare them: it is single-path, requires HEAD on a branch, and in a flat layout where the main checkout IS the slot directory its path rule ("a checkout row's path is its slot's top-level directory named by the checkout") fires before any branch rule, so a nested submodule can never be declared in place.

# Design

Remove and retire treat a nested checkout as expected when the declared checkout's `.gitmodules` names it at that path. It must sit at the gitlink commit, or have its local commits salvaged like any checkout's, and be clean; anything else still refuses. Both layouts are covered: a checkout in a top-level slot directory, and the slot directory itself as the checkout. Python and Rust stay equal. No new persisted field is introduced; any new persisted field would be reported to the coordinator before gating and gated by `writer_ceiling >= 3` (the #1788 mechanism) or not persisted.

# Acceptance Criteria

- Hermetic tests with a real temp repo and submodule, for both layouts, show remove and retire accepting a declared submodule at its gitlink (detached), one with salvaged local commits, and refusing a dirty or undeclared nested checkout.
- Python and Rust editions agree on the same cases.
- Existing remove/retire suites pass.

# Notes

Whether current main already changes this behaviour is checked before the fix and recorded in the resolution.
