---
title: 'wrkslots-sandbox-write-defaults: sandbox write-defaults handler in Rust'
status: open
priority: 2
issue_type: task
created_at: 2026-11-11T10:00:00.000000000+00:00
updated_at: 2026-11-11T10:00:00.000000000+00:00
---

# Description

P9 sandbox parity: Python `wrkslots sandbox write-defaults`
fills missing `configuration.sandbox` keys with defaults under
the config lock and reports what it added. The Rust edition
has `merge_defaults` but no dispatch-shaped handler combining
it with the raw-config checks and the printed report.

# Design

- A handler in the existing `sandbox_state` module: glue
  supplies the parsed raw config and the config path; the
  handler returns exit/stdout/stderr plus the updated raw
  config when keys were added, so the glue writes it under the
  same config lock Python uses.
- The `configuration.sandbox must be an object` refusal and
  the added/already-complete lines copy Python
  `_cmd_sandbox_write_defaults` byte for byte; merge failures
  surface through the same refusal rendering.

# Acceptance Criteria

- In-crate oracle tests compare the handler against Python
  `_cmd_sandbox_write_defaults` on the same raw configs:
  absent section, partial section, complete section, a
  non-object section, and a partial `limits` object.
- Pure data in and out; no file is written by the handler or
  its tests.

# Notes

Part of #562 rust-primary-wrkslots P9. Follows #1317. The
CLI-level `test_*_rust.py` differential lands with mu-rswrk2's
dispatch wiring, as with the earlier handlers.
