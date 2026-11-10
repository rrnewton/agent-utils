---
title: "rust-remove-census-root-maps: the Rust remove census reads cwd, exe and fd but not root or maps"
status: open
priority: 2
issue_type: bug
assignee: cl-census
created_at: 2026-11-10T00:30:00.000000000+00:00
updated_at: 2026-11-10T00:30:00.000000000+00:00
---

# Description

Found during #1173 census-vanished-process. The Rust edition's remove and audit census snapshot
(`rs/wrkslotsd/src/remove_write.rs`) reads each process's `cwd`, `exe` and `fd` entries, but not `root` or
`maps`. The Python remove census reads all five. So the Rust edition can miss a process whose only use of a slot
is a chroot root inside the slot, or a mapped file from the slot (including a deleted-but-mapped file), and
declare the slot unused.

Exposure: the Rust `remove` refuses per-slot image slots, and consumers currently run the Python remove path. But
plain worktree-slot removal in the Rust edition (the soak path) is exposed, and the gap must close before that path
is relied on.

# Design

Read the same per-process `/proc` entries as Python (cwd, exe, fd and its links, root, maps), with the same path and
(dev, ino) matching and the same error classification as #1173 (ESRCH/ENOENT = gone; EACCES and others =
indeterminate).

# Acceptance Criteria

- The Rust census reads cwd, exe, fd, root and maps.
- A differential test (both editions) where a process uses the slot ONLY through a chroot root, and another ONLY
  through an mmap of a file in the slot: both editions refuse to remove the slot.
- Existing remove differentials still pass.

# Notes

Filed for the lead coordinator; cl-census takes it right after #1173, as its own commit.
