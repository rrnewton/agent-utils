---
title: 'wrkslots-kernel-atomic-view: build the root view kernel-atomically (recursive RO first, flags before move, statmount audit)'
status: open
priority: 3
issue_type: task
labels:
- wrkslots
created_at: 2026-11-19T02:21:00.247426366+00:00
updated_at: 2026-11-19T02:21:00.247426366+00:00
---

# Description

Follow-up to #1792 boxed-root-helper-fd-mounts (owner decision, thread 205): the sealed root helper ships as an accident boundary for cooperative but sloppy agents. Replace its build-then-sweep-then-audit choreography with a kernel-atomic plan: (1) make / recursive read-only FIRST (mount_setattr AT_RECURSIVE on /) instead of sweeping per-mount afterwards; (2) set each bind's final flags (read-only, nosuid, nodev) on its detached open_tree clone BEFORE move_mount, so no mount is ever writable in the view and no remount-by-path exists; (3) verify with listmount/statmount keyed by mount id (LSMT_ROOT, per-mount) instead of walking /proc/self/mountinfo by path string; (4) consider a small native (Rust/C) constructor in place of the Python ctypes helper if the plan still needs a large privileged surface. The racing same-uid corner cases from reviews 1-3 of #1792 (rename races in the sweep/audit, os.access as proof, cached-root freshness) should be impossible by construction rather than patched case by case. Until this lands, the shipped boundary is documented in the user guide's 'Limits of the box'.

# Design

Keep the sealed threat model (PR_SET_NO_NEW_PRIVS, stdin spec, trusted roots, installed-copy check). The constructor gets the same spec; only HOW the view is assembled changes. Suggested order: flags-before-move for binds and tmpfs (small, kills the remount class), recursive-RO-first for protect_system (kills the sweep), statmount audit keyed by the mount ids recorded at move time (kills path-based verification). Each step keeps the userns entry untouched.

# Acceptance Criteria

A strict build has no path-string mount and no post-build remount: every mount carries its final flags from creation; protect_system is one recursive mount_setattr; verification enumerates mounts by id (listmount/statmount) and compares against the ids recorded before each move; the three #1792 review repro classes (noatime, stale root cache, rename race) have regression tests that fail on the old choreography; userns behavior unchanged.
