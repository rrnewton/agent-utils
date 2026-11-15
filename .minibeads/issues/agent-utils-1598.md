---
title: 'remove-readonly-trees: remove and recover finish a slot holding read-only trees instead of wedging lifecycle operations'
status: open
priority: 1
issue_type: bug
external_ref: gh-289
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

wrkslots remove wedges when a slot contains a read-only tree (directories without owner write, mode 555, for example copied build or source trees). git worktree remove --force deletes the checkout's .git file and registration, then fails deleting the read-only trees; wrkslots recover then refuses ("checkout has no safe .git file to repair"), and while that journal is pending every other removal, hold, handoff, register, and adopt on the host is blocked. The only way out today is by hand: make the trees writable and delete the fenced directory by literal path.

# Design

After the fence and salvage, before deletion, make directories inside the fenced slot owner-writable (bounded walk, never following symlinks out of the fenced path, refusing on mount crossings as in _remove_ownerless_clone_tree); reuse that helper's chmod-retry pattern on the ordinary remove/recover deletion path. Recover must also handle a removal Git already started whose .git file is gone: verify the registration is gone, finish deleting the fenced directory, and complete the journal. A still-undeletable entry refuses naming the slot only, never wedging unrelated slots.

# Acceptance Criteria

- Test: a slot containing a nested mode-555 directory tree removes cleanly.
- Test: an interrupted removal of such a slot (registration and .git file already gone) finishes via recover.
- Test: another slot's lifecycle operation proceeds while the first slot is fenced.

# Notes

[watermelon-full-preview] Filed from GitHub #289 remove-readonly-trees (https://github.com/rrnewton/agent-utils/issues/289) during Request 098 triage. Related: #911 (root-owned entries, ownership rather than mode) and the ownerless-clone tree deletion in py/wrkslots/cli.py.
