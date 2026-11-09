---
title: 'wrkslots-limits-handler: dispatch-shaped Rust handler for limits show/apply/clear'
status: open
priority: 2
issue_type: task
created_at: 2026-11-09T00:30:00.000000000+00:00
updated_at: 2026-11-09T00:30:00.000000000+00:00
---

# Description

Following #1101 wrkslots-sandbox-show-config-handler, the next
dispatch-shaped handler is `wrkslots limits {show,apply,clear}`:
the machine-wide user-slice guard. Python behaviour
(`py/wrkslots/imagecmd.py:1727`): `apply` validates the fractions
in (0, 1], computes `MemoryMax` from physical memory and
`CPUQuota` from the CPU count, and runs
`sudo -n systemctl set-property --runtime user-<uid>.slice ...`;
`clear` sets the infinities; both then print the result of
`systemctl show` for the three properties, indented, with exit 0
only when the show succeeded. A failed set refuses (exit 3).

# Design

- Handler in the already-registered `sandbox_exec` module: host
  facts (uid, physical bytes, CPU count) are injected and every
  command goes through the `PrivilegedRunner` seam, so tests use a
  recording fake and nothing touches the host's real slice.
- An in-crate oracle differential (same style as #1095 and #1101)
  drives the Python handler's pure decisions against the Rust
  handler on identical facts and compares bytes, including the
  fraction refusal and a failing set.
- No shared file is edited; the dispatch arm is proposed in the
  landing report, and the CLI-level differential lands with the
  wiring.

# Acceptance Criteria

- The oracle differential passes on show, apply, clear, the
  fraction refusal, and the failing-set refusal.
- The handler lands unwired, gate green, with the dispatch arm
  text sent to the coordinator.
- No real `systemctl` mutation runs in tests.

# Notes

Part of #562 rust-primary-wrkslots P9. Following handlers in the
lane's list: image mount family, worktrees-image, migrate/convert,
run/box.
