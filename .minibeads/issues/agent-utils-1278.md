---
title: 'wrkslots-salvage-bundle-object-store: salvage bundles of submodule history see the parent module store'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-11T08:27:03+00:00
updated_at: 2026-11-11T08:27:03+00:00
---

# Description

[opus 5.5] A consumer's first slot removals with `wrkslots remove --archive-private-history` refused with rc 3, on two slots whose nested submodule lives in a per-worktree module store (`.git/worktrees/<slot>/modules/<module>`, no alternates). The error was `git bundle create` of the salvage ref for the nested module failing with `fatal: unable to read <blob>`. That blob is present in the parent checkout's module store (`.git/modules/<module>`) but not in the slot's store. `git fsck --connectivity-only` on the slot stores is clean, the blob isn't in their index, and the worktrees are clean.

The refusal was safe: nothing was deleted and the refs had been backed up first. But removals stay blocked until this is fixed. The bundle staging in `_ensure_local_salvage_bundle` and the shared-bundle path (#1153) sets `GIT_OBJECT_DIRECTORY` to the slot's common-directory object store alone, with no alternates. So a salvage commit whose tree needs objects that only the parent module store holds can't be bundled.

# Design

- Reproduce in fixtures only: a superproject with a submodule, and a linked worktree with its own module store holding a salvage-worthy commit whose tree references a blob present only in the parent module store.
- Preferred fix: stage with the parent module store as an alternate (`GIT_ALTERNATE_OBJECT_DIRECTORIES` or an `objects/info/alternates` file in the staging repository). The bundle stays a complete-history, self-contained bundle, and `git bundle verify` passes on it in an empty repository before anything is deleted.
- Fallback where no store holds every object: a precise refusal that names the commit, the missing object and each store searched.
- Never weaken the refusal for a commit that can't be bundled completely.
- Fix the Python edition and Rust `remove_write.rs` together, keeping the Rust/Python differential exact.

# Acceptance Criteria

- The fixture that refuses today removes successfully, and its salvage bundle verifies and fetches into an empty repository with the full history.
- With the object absent from every store, removal still refuses, names the missing object, and deletes nothing.
- Python and Rust produce identical journals and receipts on both fixtures.
- Tests use no wall-clock budgets.

# Notes

Reported by a consumer's coordinator on 2026-11-11, against agent-utils 963c3e9b.
