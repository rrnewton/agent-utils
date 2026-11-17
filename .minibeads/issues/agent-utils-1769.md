---
title: 'wrkslots-census-exiting-process-indeterminate: an exiting process with an absent mount-namespace link is judged exited by condition, not by a timed window'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T07:38:00-05:00
updated_at: 2026-11-17T07:38:00-05:00
---

# Description

[watermelon-full-preview] During `recover --slot <validate slot>`, the
same-UID process census refused:

"mount namespace is indeterminate for live PID N after 4 re-probes over
150 ms: its mount-namespace link stayed absent but the process was not
proven exited in that window"

The PID was a process exiting during the scan, not a user of the
checkout. The identical recover rerun a few minutes later succeeded.
Path: `_cmd_recover` -> `_recover_finish` -> `_finish_remove_paths` ->
`_capture_same_uid_process_path_census` ->
`_same_uid_process_observations` -> `_mount_namespace_reprobed`. This
follows #875 wrkslots-census-exiting-pid and #906, which added the
bounded re-probe; the bound itself is now the defect when an exit
takes longer than the window to reach a provable state.

# Design

- A process whose `/proc/<pid>/ns/mnt` link reads as absent is
  exiting; judge it exited (not indeterminate) when `/proc/<pid>`
  vanishes, its stat state is Z or X, or its start time / identity
  no longer matches. Reuse the stat state parsing that made zombies
  dead in seal liveness (#1736). Re-check those conditions after the
  last re-probe, before refusing.
- The verdict comes from conditions, not a wall-clock window
  (owner rule 346): replace the fixed re-probe count and delay with
  an exit wait (pidfd open plus poll for exit where available;
  otherwise re-probe until the process is proven exited or proven
  to have a namespace), with a generous backstop that refuses only
  when the process is still alive with no namespace at the end.
- Keep fail-closed for a live process whose namespace really cannot
  be read. Keep the change inside the census helpers so neighbouring
  remove/census work is untouched. The Rust edition is changed only
  if it has this census path.

# Acceptance Criteria

- Deterministic tests with fake /proc snapshots, no real sleeps:
  (a) namespace link absent then `/proc/<pid>` gone: exited, census
  proceeds; (b) namespace absent then state Z: exited; (c) namespace
  absent, process alive and identity unchanged through the backstop:
  still refuses (fail closed); (d) an unreadable (permission) namespace
  on a live process: refuses as today.
- An exiting process that takes longer than the old 150 ms window to
  become provable no longer refuses the census.
- Single-process and other census behaviour is otherwise unchanged.

# Notes

[watermelon-full-preview] Filed by mu-censusexit. Prior work: #875
added the bounded re-probe this issue replaces; #1736 made stat
state Z/X dead in seal liveness in both editions.
