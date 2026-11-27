---
title: 'wrkslots-private-module-retire-and-rust-parity: retire tests and Rust parity for the private-module judgment'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
- lifecycle
created_at: 2026-11-26T12:00:00+00:00
updated_at: 2026-11-26T12:00:00+00:00
---

# Description

[watermelon-full-preview] Follow-up from #2168 wrkslots-declared-submodules-as-expected-nested (landed bce0b640): the private-module judgment added there is exercised through remove, which retire shares, but has no dedicated retire tests, and the Rust edition has no twin of the judgment (its remove refuses these salvage corners explicitly), so full Python/Rust parity is unproven.

# Design

Add hermetic retire tests for a declared submodule placed as a shared linked worktree (accepted at the gitlink, refused unreadable/dirty), and either port the nested-module judgment to the Rust remove or record explicitly, with differential tests, where the editions intentionally differ. No new persisted field is expected; any new persisted field would be reported to the coordinator before gating and gated by `writer_ceiling >= 3` (the #1788 mechanism) or not persisted.

# Acceptance Criteria

- Retire accepts and refuses the same declared-submodule shapes as remove, in hermetic temp-repo tests.
- Python and Rust behaviour on those shapes is either equal or the difference is tested and documented.

# Notes

Filed at #2168 close time as agreed with the coordinator.
