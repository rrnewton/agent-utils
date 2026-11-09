---
title: "gate-soak-host-optin-isolation: a per-host Rust readers opt-in leaks into gate test sandboxes"
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-09T22:37:10.000000000+00:00
updated_at: 2026-11-09T22:37:10.000000000+00:00
---

# Description

The Rust readers soak can be opted in per process with
`WRKSLOTS_RUST_READERS=1` or per host with an `enabled = true` line in
`~/.local/etc/agent-utils/wrkslots-rust-readers`. `scripts/gate.py` passes the
calling process environment to the admitted validation command and preflight,
and resolver tests inherit the real `$HOME`, so enabling the soak on one host
changes default dispatch inside gate test sandboxes. A sandbox tree without
`common/bin/wrkslots-soak` paged the 2026-11-09 soak at 11:09 PM ET when
`test_python_only_tool_defaults_to_python` exited 127 on the missing launcher.

# Design

Make `scripts/gate.py` the isolation boundary. It builds one child
environment for both the preflight and the admitted command: preserve the
caller's environment, add `GATE_ADMISSION_RUN_ID`, and force
`WRKSLOTS_RUST_READERS=0`. It also points `WRKSLOTS_RUST_READERS_FILE` at a
nonexistent path under the gate scratch directory. `engine-resolver` already
gives `WRKSLOTS_RUST_READERS=0` precedence over the file, and tests that
deliberately exercise routing replace those environment values afterwards.
Real interactive calls outside `gate.py` continue to honour the host file.
Keep the existing test-helper pins as a second layer. Also add a gate test in
which an enabled host file is present while a child echoes its routing
environment; the child must observe the forced opt-out values.

# Acceptance Criteria

- A gate child sees `WRKSLOTS_RUST_READERS=0` even when the parent process has
  the variable set to `1`.
- A gate child sees `WRKSLOTS_RUST_READERS_FILE` pointing to a path that does
  not exist, even when the parent process points it elsewhere.
- The admitted command still receives `GATE_ADMISSION_RUN_ID`.
- A sandbox resolver call with a real enabled host file stays on Python under
  the gate environment, and the dedicated resolver tests still cover explicit
  opt-in routing outside that isolated child environment.

# Notes

Approved by the lead coordinator on 2026-11-09 after the first reader-soak
pause. Build the test helpers' existing pins remain as defence in depth.

## Resolution

[watermelon-full-preview] `scripts/gate.py` now has a single
`_child_environment` helper used by both `run_preflight()` and
`run_admitted()`. It preserves the parent environment while forcing
`WRKSLOTS_RUST_READERS=0` and pointing `WRKSLOTS_RUST_READERS_FILE` at a
nonexistent path under the gate admission directory. A gate admission test
sets an enabled host file and parent `WRKSLOTS_RUST_READERS=1`, then proves the
admitted child observes the opt-out values and keeps its admission run ID.
