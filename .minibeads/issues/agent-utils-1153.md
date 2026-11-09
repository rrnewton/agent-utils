---
title: "remove-salvage-duplicate-bundles: removal writes ~460 MB of full-history bundles for a fully published slot and holds the journal ~500 s"
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-09T18:05:00.000000000+00:00
updated_at: 2026-11-09T19:00:00.000000000+00:00
---

# Description

Measured on 2026-11-09 by the first adopter of the worktrees image (a consumer), removing a typical reviewer slot.
`wrkslots remove` wrote about 460 MB of salvage bundles and held the ACTIVE finish journal for about 500 s. Local
validation entry refuses to start while that journal is outstanding (#1108 and the finish-journal busy-vs-interrupted
proposal), so one removal cost a merge-queue job a deferral and blocked an hourly main validation.

Causes:
1. Superproject, 385 MB: the source HEAD is on the remote, but the checkout counts as dirty only because a submodule
   gitlink points at another commit (` M <submodule>`), and that commit is itself published. A salvage commit that
   is not on any remote is created, so a complete-history bundle is written and verified (fresh bare clone plus a
   full fsck).
2. Two clean pinned submodules, 63 MB and 16 MB: their status digest is empty, but their pinned commits are not
   inside any CURRENT remote branch head, so `remote_heads_containing` returns nothing and they are bundled. Every
   slot carries the same pins, so every removal writes the same 79 MB again.

`salvage_push_remotes` is not a fix. It pushes `refs/salvage/...` refs to the remote, which clutters the remote and
spends the push budget. Without egress the push fails, and with no `--salvage-archive-root` the removal refuses
(`_salvage_one_checkout`: `except Refusal: if archive_root is None: raise`).

# Design

Any combination:
- (a) A superproject whose only difference from its published HEAD is gitlinks pointing at commits that are
  themselves published counts as already published: no salvage commit and no bundle. The proof checks each
  gitlink's commit against its own submodule remote.
- (b) Reuse an existing verified bundle for the same (repository, salvage commit) instead of writing another; record
  the reuse in the receipt.
- (c) Thin bundles whose prerequisites are proven present on the remote, verified by unbundling into a scratch clone
  of the local primary.
- (d) For pinned submodule commits, accept containment in fetched tags and remote-tracking refs, not only current
  remote branch heads.

Safety bar: never drop salvage for a commit that is not provably on a remote. Every "published" proof names the
remote ref that contains the commit, and the receipt records it.

# Acceptance Criteria

- With (a) and (b), removing a fully published reviewer-style slot (gitlink-only dirt, clean pinned submodules
  already bundled once) writes no new bundle, and the journal hold drops to about 60 s or less. Measure before and
  after.
- A slot with a truly unpublished commit (in the superproject or a submodule) is still salvaged. Tests cover a
  gitlink pointing at an unpublished submodule commit, and a pinned commit reachable only from a deleted branch.
- Receipts name the proof (remote, ref, commit) for each skipped bundle.
- Rust parity filed or done where the Rust edition implements remove salvage.

# Notes

Reported by the first adopter of the worktrees image (2026-11-09). Same remove-path and journal-hold family as
#1108 and #1142 image-remove-fence-first.

## Resolution

[opus 5.5] (a), (b) and (d) are done in the Python edition. (c) is deferred: after (b) a pin's
bundle is written once per commit, not once per slot (measured below), so no follow-up is filed.

Every skipped bundle names live remote proof in its receipt (`publication_proof`) and is asked of
the remote again before deletion:
- (a) `published-gitlinks`:
  - HEAD is on a live remote branch;
  - the salvage tree differs from HEAD's only in gitlinks modified in place (mode 160000 on both
    sides);
  - each gitlink commit is the HEAD of an initialized submodule of the same removal, held by that
    submodule's recorded remote in a live branch or tag.

  The receipt's `gitlinks` lists path, commit, checkout, remote and the containing refs and tags,
  so HEAD plus the list rebuilds the salvage tree.
- (d) `remote-tag`: a clean submodule HEAD on no live branch but in a tag the remote lists now. A
  local refs/remotes/* or tag ref never counts.

  The tags go in a new receipt key, `containing_remote_tags`. `containing_remote_refs` keeps its
  refs/remotes/<remote>/ rule in both editions.
- (b) A shared bundle per commit at `wrkslots-salvage/_by-commit/<commit>/`:
  - it is verified in full once, and `bundle.json` is written only after that verification passes;
  - a later removal reuses it only when the bundle's SHA-256 and size match `bundle.json` and the
    record says the verification passed;
  - each slot keeps its own receipt with `shared_bundle` and `bundle_reused`;
  - the removal's prefetch makes or reuses the same bundle.

The lead's three conditions:
1. Concurrent writers. An exclusive flock on `_by-commit/<commit>/.lock` serializes them. The
   second writer finds `bundle.json` and reuses the bundle. Nothing is overwritten: a bundle or
   record that disagrees refuses reuse, and that checkout keeps a bundle of its own. The
   two-process race test checks for exactly one bundle, with one writer reporting reused=False
   and the other reused=True.
2. Retention. NOTHING DELETES `_by-commit/`. That is stated at `_SHARED_SALVAGE_DIRECTORY` and in
   the USER_GUIDE: any future pruning must first count every receipt that names a bundle.

   Audit:
   - the only deletions in the salvage code are of temporary bundles;
   - a salvage archive root must be the control directory or lie outside the project tree;
   - clean-caches skips `wrkslots-salvage` in the control directory (`_REPRESENTATION_DIRECTORIES`).

   The reuse test runs clean-caches --yes and finds the shared bundle and its record unchanged.
3. Fallback. The check before the path fence leaves a lost proof, or a shared bundle that
   disagrees with its record, to the decisive check after the fence. That check keeps the salvage
   commit in a verified bundle of the slot's own (`archive_reason` `publication-proof-lost`) and
   journals that receipt in place of the old one before anything is deleted, so the archive record
   names a real bundle. Tested with a remote tag deleted after the fence and with a shared bundle
   changed after the fence.

Measured on devbig030 by day, on a fixture: a 30 MB superproject whose only change is a gitlink at
a published commit, plus two submodules (20 MB and 5 MB) pinned at commits no live branch holds,
with two slots removed in turn (script /tmp/cl-wrkid-1153/measure.py):

| Slot | Bundle bytes before | After | `remove` before | After |
|---|---|---|---|---|
| first | 57.7 MB | 26.2 MB (the two pins, once) | 34.6 s | 29.4 s |
| second | 57.7 MB | 0 MB (reused) | 32.5 s | 28.1 s |

The finish-journal hold was about 9 s both ways on this fixture, because the removal's prefetch
makes the bundles before the journal is written. The adopter's 500 s hold had bundle work inside
the journal, which this fixture does not reproduce. With (a) and (b) that work is gone for a fully
published slot whose pins were bundled once.

Rust parity: the Rust readers treat salvage receipts as opaque mappings (archive_state.rs,
journal_state.rs), so the new keys parse. Rust remove salvages only clean, already-published
checkouts and refuses dirty or unpublished ones, so for (a) and (d) it stays STRICTER: it refuses
where Python now skips, which is safe. (b) is Python-only, because Rust never writes bundles. This
is recorded here instead of a port.

