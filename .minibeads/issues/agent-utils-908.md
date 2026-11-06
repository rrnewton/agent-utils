---
title: 'wrkslots-status-quiet-pidns: status reports tearing-down PID namespaces only with --verbose'
status: closed
priority: 3
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-05T18:26:05.695118146+00:00
updated_at: 2026-11-05T18:26:05.695118146+00:00
closed_at: 2026-11-06T19:18:43.000000000+00:00
---

# Description

Since #879 (c5dc908e), every `wrkslots status` prints about eight informational stderr lines on a host with lingering PID-namespace inits:

    PID N (exec_test) is a PID-namespace init being torn down (wchan zap_pid_ns_processes ...) so it cannot hold a checkout; not counted as an occupant

The exit status stays 0, but any caller that treats stderr as failure trips on it, and it is noise. The processes involved on the lead's host are eight `[exec_test]` PID-namespace inits stuck in `zap_pid_ns_processes` for more than eight days (state SN), leaked by some sandboxed test exec. agent-utils itself does not spawn `exec_test`; a search of the tree finds no such name.

# Design

- Print the per-PID explanation only with `--verbose`, or in diagnostics output. By default, print at most one summary line per run, "N tearing-down PID namespaces ignored", or nothing.
- The classification itself stays: such PIDs are still not counted as occupants.
- Python and Rust editions emit the same thing.

# Acceptance Criteria

- Test: with tearing-down PID-namespace inits present, a default `wrkslots status` writes no per-PID lines to stderr (at most one summary line), and `--verbose` shows the per-PID detail.
- The status differential still agrees.

# Notes

Reported by the lead on 2026-11-05 at 1:25 PM ET (P3), from an install of 0c560e1d on that host. Assigned to mu-validate, who wrote #879, at the end of its queue. Do not kill those processes by pattern. If a wrkslots or dagrun test ever spawns a PID-namespace init, check that its cleanup cannot wedge in zap_pid_ns_processes.

Moved from mu-validate to the new Muse ultra wrkslots lane mu-wrkslots on 2026-11-05 at 5:50 PM ET (the owner lifted the Muse lane cap; the lead asked for a dedicated wrkslots lane).

## Resolution

Landed in the mu-wrkslots stack at 84585684. `wrkslots status` counts tearing-down PID namespaces in one summary line instead of narrating each PID; `status --verbose` restores per-PID detail. Classification unchanged (still not occupants); the Rust edition never narrated them.
