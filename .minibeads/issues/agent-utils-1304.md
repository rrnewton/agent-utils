---
title: 'wrkslots-run-place-only: run --place-only handler in Rust'
status: open
priority: 2
issue_type: task
created_at: 2026-11-11T10:00:00.000000000+00:00
updated_at: 2026-11-11T10:00:00.000000000+00:00
---

# Description

P9 run/box parity, first dispatch-shaped step: Python
`wrkslots run --place-only` applies the slot's slice limits,
prints the placement (human or JSON), and runs nothing. The Rust
edition has the pure pieces (`placement`, `slice_application_plan`,
`run_slice_plan`) but no handler that combines them with the
command's option validation.

# Design

- A dispatch-shaped handler in the existing `sandbox_exec`
  module: glue supplies slot type, slot, limits, injected cgroup
  text, output format, and the given-option flags; effects go
  through the `PrivilegedRunner` seam (fake runners in tests).
- Option refusals copy Python `_cmd_run`/`_place_only` byte for
  byte: `--place-only runs nothing and takes no ...` for COMMAND,
  `--print`, `--cwd`, and for `--in-scope`, `--expect-slices`,
  `--expect-writable`, `--git-config-policy`, `--expect-masks`,
  and `--closed-set` given alongside it.
- Applied limit lines go to stderr as `wrkslots run: limits ...`;
  the placement prints as the three human lines or the sorted
  JSON object. Placement is derived once from the injected
  cgroup text and the limits are applied to exactly those names.
- Execution modes of `run`/`box` (scope entry, namespace entry)
  stay later steps; this handler runs nothing, as Python's does.

# Acceptance Criteria

- In-crate oracle tests compare the Rust handler against the
  Python `_place_only`/`place_only` behaviour on the same facts:
  no enclosing slice, an enclosing slice, JSON and human formats,
  limits applied and no limits configured.
- Each refused option combination returns exit 3 with Python's
  refusal text, and a failing set-property refuses with
  `cannot set limits on <unit>: ...`.
- Fake-runner tests only; no real systemctl, scope, or cgroup is
  touched.

# Notes

Part of #562 rust-primary-wrkslots P9. Follows #1291. The
CLI-level `test_*_rust.py` differential lands with mu-rswrk2's
dispatch wiring, as with the earlier handlers.
