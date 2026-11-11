---
title: 'wrkslots-run-in-scope-plan: run --in-scope --print handler in Rust'
status: open
priority: 2
issue_type: task
created_at: 2026-11-11T10:00:00.000000000+00:00
updated_at: 2026-11-11T10:00:00.000000000+00:00
---

# Description

P9 run parity: Python `wrkslots run --in-scope UNIT
--expect-slices ... --print` verifies the caller is inside
the named scope under the expected slices, then prints what
entering the slot's box would do (view spec, writable set,
masks, helper argv) without entering it. The Rust edition has
the placement and spec pieces but no `verify_in_scope`,
`writable_set`, `mask_report`, or in-scope plan handler.

# Design

- `writable_set`, `mask_report` and `planned_private_sources`
  in the existing `sandbox_state` module, and
  `verify_in_scope` plus a dispatch-shaped
  `run_in_scope_print` handler in `sandbox_exec`, mirroring
  Python `sandbox.py`/`imagecmd.py` byte for byte.
- The handler parses the expectation texts itself so their
  refusals match: `--in-scope and --expect-slices are given
  together`, the closed-set/`--no-protect-system` refusal, the
  `--expect-slices` shape checks, and the whole-object
  writable/masks comparisons with Python's refusal texts.
- Facts are injected (home, the cgroup file path, helper
  interpreter/module); the cgroup file is read from the given
  path exactly as Python reads it. Entering the box
  (`enter_in_place`, exec) stays a later dispatch-time step.

# Acceptance Criteria

- In-crate oracle tests compare the handler against Python
  `_in_scope`/`in_scope_plan` on the same fixture: a clean
  plan, expectations accepted, a tampered writable set
  refused, a caller in the wrong process scope refused, a
  non-root isolation refused, and the two `_in_scope` early
  refusals.
- Fixture trees and files only; no scope is started, no
  namespace is entered, nothing is mounted.

# Notes

Part of #562 rust-primary-wrkslots P9. Number assigned by
coordb from the devbig014 block. Follows #1317 and the
sandbox write-defaults handler. The CLI-level
`test_*_rust.py` differential lands with mu-rswrk2's dispatch
wiring, as with the earlier handlers.
