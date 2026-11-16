---
title: 'wrkslots-finish-discard-ignored: finish must handle ignored-only content'
status: closed
priority: 2
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-15T21:00:00+00:00
updated_at: 2026-11-16T19:30:00+00:00
---

# Description

[watermelon-full-preview] Queued from a consumer report (owner 6539) via coordb, after
#1582. `wrkslots finish` refuses a slot whose checkout has a non-empty `ignored/`
("checkout <name> is dirty or has untracked/ignored files (! ignored/)") and documents no
way to discard ignored-only content (`--discard-unaccounted` covers only unlanded commits).

# Design

- Add a documented `--discard-ignored` to `finish` that archives (default) or deletes
  ignored-only content and records it in the finish receipt; untracked non-ignored files
  still refuse.
- Removing a provisioned path (e.g. a `.buckconfig.local` symlink) makes finish's
  provisioned-path check refuse; make its message name the path and how to restore it.
- Tests for both. Git only (Eden freeze until 6:30 AM ET).

# Notes

Landed on main as 00f5bd02 (gated tree a2f2a7ae). `finish` gained
`--archive-ignored` and `--discard-ignored` (mutually exclusive, Git
only): ignored-only checkouts finish, the handoff records every ignored
file (checkout, path, bytes, sha256, disposition) plus the verified tar
when archived, and `remove` enforces that exact record. Tracked or
untracked work still refuses. A changed or deleted provisioned path now
names its recorded state and how to restore it. Python and Rust editions
in parity; gate red only on five untouched steps, all passed solo reruns.
