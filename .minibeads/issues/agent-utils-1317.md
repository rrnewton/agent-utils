---
title: 'wrkslots-run-box-print: SlotView, build_spec and run/box --print in Rust'
status: open
priority: 2
issue_type: task
created_at: 2026-11-11T10:00:00.000000000+00:00
updated_at: 2026-11-11T10:00:00.000000000+00:00
---

# Description

P9 run/box parity, second step: Python `sandbox.run` with
`print_only` prints the boxed command without running it, for
all three isolations. It needs `SlotView` and `build_spec`,
which the Rust edition does not have yet: the view spec (binds,
masks, read-only roots, home layer) is computed nowhere in
`wrkslotsd`.

# Design

- `SlotView` and `build_spec` in the existing sandbox modules,
  mirroring Python `sandbox.py`: `path_aliases` from an injected
  mountinfo text, `home_entries`, `expand_path`, the
  gate-admission bind, the root-launcher spec directory check,
  and the isolated Git configuration masks. Host facts (user,
  uid, mountinfo, runtime directory, gate directory) are
  injected; filesystem existence checks read the caller's
  fixture or real tree exactly as Python's do.
- A dispatch-shaped `run_print` handler on top: Python's check
  order (isolation, no command, systemd-run availability), the
  `# isolation ...; scope ... in ...` header, slice and env
  comment lines, and the cgroup/userns/root renderings with
  Python `shlex.quote`. Glue supplies the final view (box
  `--cwd` and agent-registry handling stay with glue, as does
  substituting `$SHELL` for an empty command).
- Execution itself (scope entry, namespace entry, exec) stays a
  later dispatch-time step; `--print` runs nothing in either
  edition.

# Acceptance Criteria

- In-crate oracle tests compare Rust `build_spec` against
  Python `build_spec` on the same fixture `$HOME` and views:
  agent and coordinator views, `home: ro` and `home: hidden`,
  writable and closed-set (`slot_writable=False`), isolated Git
  policy, the `HOME=/` refusal, and a bad spec-directory mode.
- Oracle tests compare `run_print` output byte for byte for
  cgroup, userns and root isolations, plus the no-command and
  no-systemd-run refusals.
- Fixture trees and fake runners only; nothing is mounted,
  no scope is started, and no real `$HOME` is listed.

# Notes

Part of #562 rust-primary-wrkslots P9. Follows #1304. The
CLI-level `test_*_rust.py` differential lands with mu-rswrk2's
dispatch wiring, as with the earlier handlers.
