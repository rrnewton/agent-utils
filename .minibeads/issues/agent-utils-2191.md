---
title: 'wrkslots-root-box-hangs-without-tty: a root box started with no terminal never runs its command'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- sandbox
assignee: mu-rootbox
created_at: 2026-11-24T05:45:00-05:00
updated_at: 2026-11-24T05:45:00-05:00
---

# Description

`wrkslots box --isolation root --cwd worktrees -- COMMAND`, started from a tool call with no terminal, prints the two `wrkslots run: limits ...` lines and then never runs COMMAND: no probe file inside or outside the writable set is created. A `timeout 90` wrapper did not end it in the reporting harness; it was stopped by hand at about two minutes, leaving no box scope behind.

Reproduction on a throwaway project shows this is not a deadlock: the root helper's strict view build is quadratic in (home entries x mount-table size). `enter_root` -> `build_view(strict_fd=True)` binds every top-level $HOME entry read-only (about 500 entries on the reporting class of host). Per bind, `_bind_fd_strict` read and parsed all of /proc/self/mountinfo twice (`_mount_ids_by_point` before and after), and `_remount_tree_read_only` parsed it again via `_mounts()`. Measured against a real captured spec with an instrumented helper copy: 515 binds took 38s and 503 tree remounts took 20s, 60s total before the command exec'd. On a larger home or mount table the build exceeds any reasonable wrapper timeout, so the launch looks hung and the command never starts. Test fixtures pass because they use tiny homes and mount tables.

A second, independent wedge on the same path: `place_in_scope` calls `busctl` with no timeout, so a user manager that never answers hangs the launch before anything runs, with the same outward symptom.

# Design

In `py/wrkslots/sandbox.py` (box/run are Python-only; the Rust sandbox code only builds argv plans):

- Read the mount table ONCE per strict build into a build-scoped cache (`_strict_mount_ids`), extended with every mount the build itself makes via `_note_strict_mount`. `_bind_fd_strict` proves a bind landed from the mount id `_bind_resolved_fd` already returns: an id already known at the target before the move refuses (fail closed), never silently trusted. The final protect_system sweep drops the cache and re-reads fresh, and the read-only audit keeps its own fresh read, so a mount arriving from the host mid-build is still caught.
- Bound `place_in_scope`'s `busctl` call (timeout=10, matching `scope_in_place_available`); on timeout the cgroup poll still decides, otherwise the caller falls back to `systemd-run --scope` as today.

No new persisted fields. The installed root helper is root-owned host state; reinstalling it after the fix lands is separate root work for the host owner, not part of this change.

# Acceptance Criteria

- A root box started with no terminal runs its command promptly on a host with hundreds of home entries and a large mount table.
- A write outside the box's writable set fails and a write inside it succeeds (manual proof against the real installed helper on a throwaway project).
- A regression test bounds the total mount-table reads per strict build to a constant independent of the home entry count (fails before the fix at 124 reads for 40 binds, passes after).
- A regression test proves a bind whose mount id was already known at its target is refused.
- The box exits promptly when its launcher receives SIGTERM.

# Notes

[watermelon-full-preview] mu-rootbox. Owner priority: the root sandbox must work so agents can start with their filesystem view sandboxed. Related: #1583 (root helper trust boundary), #1792 (fd-based strict build). The test-hermeticity fallout from installing the helper on this host is handled as a separate small commit ahead of this fix.
