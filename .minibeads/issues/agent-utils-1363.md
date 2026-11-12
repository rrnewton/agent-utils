---
title: 'wrkslots-sandbox-prepare-state: prepare_state and child_environment in Rust'
status: open
priority: 2
issue_type: task
created_at: 2026-11-12T10:00:00.000000000+00:00
updated_at: 2026-11-12T10:00:00.000000000+00:00
---

# Description

P9 run/box parity: before a boxed command starts, Python
`sandbox.prepare_state` builds the slot's private `$HOME`
layer (placeholders, recreated links, seeded private files,
private directories, stale-placeholder cleanup) and
`sandbox.child_environment` computes the child's whole
environment. The Rust execution entry (a later dispatch-time
step) needs both from the sandbox modules.

# Design

- `prepare_state` in the existing `sandbox_state` module,
  operating on the caller's tree exactly as Python's does:
  the layer directory, the `home-placeholders.json` record,
  placeholder/link/file creation, and stale removal; the
  "is in the way" warnings are returned for the caller to
  print, as Python prints them to stderr.
- `child_environment` (with `forwarded_environment`) beside
  it: the denylist, private-`/tmp` handling, identity keys
  for slot and coordinator views, and configured `env`
  expansion, preserving Python's dict-assignment order.
- Both take injected facts where Python reads process state
  (the environment list, `$HOME`, the process id); filesystem
  effects happen only under the given state directory.

# Acceptance Criteria

- In-crate oracle tests compare `prepare_state` against
  Python on identical fixture trees: first build, a second
  idempotent build, and stale-placeholder removal, comparing
  the resulting trees, the record contents, and warnings.
- Oracle tests compare `child_environment` byte for byte
  (ordered pairs) for slot and coordinator views in userns
  and cgroup modes.
- Fixture trees only; the real `$HOME` and live slot state
  are never touched.

# Notes

Part of #562 rust-primary-wrkslots P9. Number assigned by
coordb from the devbig014 block. Follows #1333. The
CLI-level `test_*_rust.py` differential lands with mu-rswrk2's
dispatch wiring, as with the earlier handlers.
