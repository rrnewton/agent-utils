---
title: 'wrkslots-salvage-bundle-object-store: salvage bundles of submodule history see the parent module store'
status: closed
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

## Resolution

[watermelon-full-preview] The salvage commit is the octopus commit private-state salvage builds in the slot's per-worktree module store: its first parent is the submodule HEAD and the rest are the kept private-history tips. Those kept commits can reach blobs no store in the slot holds — some only in the parent checkout's module store, some in no searched store at all because a partial (promisor) clone never fetched them, which is why connectivity-only fsck stays clean.

Bundle staging in both `_ensure_local_salvage_bundle` and `_ensure_shared_salvage_bundle` now stages with the parent module store and the stores the checkout's own alternates name as read-only alternates (`GIT_ALTERNATE_OBJECT_DIRECTORIES`), always with `GIT_NO_LAZY_FETCH=1`, so the bundle stays complete-history and self-contained and still verifies in an empty repository before anything is deleted. Before the salvage commit is built, each kept commit is checked against those stores: one whose missing objects are all promisor gaps is excluded and recorded in the salvage receipt (and the bundle receipt) under `excluded_incomplete_history` as {commit, missing_object, "already-incomplete history, promisor gap"}, with the count printed before deletion; one reaching any other missing object refuses, naming the kept commit, the missing object, and every store searched, and deletes nothing. The Rust edition's `salvage_gate` writes the same receipt field (empty for the clean published checkouts it handles), so the Rust/Python differential stays exact and no archive-record key was added. Tests cover all three fixture shapes in `test_lifecycle.py`: a blob only the parent store holds (bundles and restores), a promisor-gap kept commit beside a complete one (excluded, recorded, the rest bundles), and a blob missing from every store (refuses, naming commit, object, and both stores). The kept-set over-collection behind the reported 3,870 kept parents is spun out to #1280 wrkslots-kept-set-over-collection.
