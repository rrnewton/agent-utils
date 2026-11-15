---
title: 'wrkslots-finish-discard-ignored: finish must handle ignored-only content'
status: open
priority: 2
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-15T21:00:00+00:00
updated_at: 2026-11-15T21:00:00+00:00
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
