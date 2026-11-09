---
title: 'wrkslots-root-through-symlink: check that wrkslots called through a symlinked launcher still finds the harness root'
status: closed
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-06T22:36:35.000000000+00:00
updated_at: 2026-11-09T06:50:00.000000000+00:00
closed_at: 2026-11-09T06:50:00.000000000+00:00
---

# Description

A consumer calls wrkslots through a `bin/wrkslots` symlink. From restarted lanes, its root resolves to the parent work directory instead of the harness, so transfer-owner refuses. The workaround is to call it by absolute path.

The Python edition's `_discover_root` searches from the current directory upward for `.wrkslots.yml`, not from the launcher's location, so the Python CLI alone should not depend on the symlink. Check the other paths: the `common/bin/wrkslots` launcher, the Rust `wrkslotsd` edition, and the soak wrapper. Each must resolve the root the same way, by `--root`, then the cwd upward, never from the resolved location of the executable.

# Acceptance Criteria

- Tests run through a symlinked launcher from a slot directory: every edition finds the harness root.
- A fix, if any edition resolves from the executable's path.

# Notes

Reported 2026-11-06 by the lead coordinator from a consumer lane.

## Resolution

Checked by cl-wrkid, 2026-11-08 ET. No edition or launcher resolves the project root from the executable's location, so there is nothing to fix in agent-utils. Tests were added to keep it that way.

- Python edition: `_discover_root` takes `--project-root`, then the working directory (physical, `Path.cwd().resolve()`) and its parents. It never looks at its own path.
- Rust edition: `config_validate::discover_root` does the same with `--root`, then the canonical `current_dir()` and its parents.
- `common/bin/wrkslots` (engine-resolver) and `common/bin/wrkslots-soak` resolve their own real path only to find the agent-utils checkout, in a subshell, and exec the edition from the caller's directory. The soak adds `--root` only when the caller passed `--project-root`.
- `rs/bin/cargo-runner` changes to the checkout to build, then returns to the caller's `$PWD` before it execs the binary.

Tests:

- py/tests/test_wrkslots_soak.py `test_every_route_keeps_the_callers_directory_through_a_symlinked_launcher` uses stub editions. A harness's `bin/wrkslots` symlink is called from a slot directory through each route: the Python primary, `WRKSLOTS_ENGINE=rust`, `rust-soak`, and the readers soak. In every route the edition runs in the slot directory and gets no `--root`.
- py/wrkslots/tests/test_doctor_audit_cli_rust.py `test_both_editions_find_the_harness_root_from_a_slot_through_symlinks` uses the real editions. Both are called through symlinks, from a slot reached through a symlinked path, and both report the harness root.

So the consumer's symptom points at the working directory its restarted lanes start in, not at the symlink. A lane that starts above the harness, for example in the parent work directory, finds whatever `.wrkslots.yml` lies upward from there. For the lead to pass on: have that launcher pass `--project-root <harness>`, or restart lanes in their slot directory.

