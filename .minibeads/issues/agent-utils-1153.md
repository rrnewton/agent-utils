---
title: "remove-salvage-duplicate-bundles: removal writes ~460 MB of full-history bundles for a fully published slot and holds the journal ~500 s"
status: open
priority: 2
issue_type: bug
created_at: 2026-11-09T18:05:00.000000000+00:00
updated_at: 2026-11-09T18:05:00.000000000+00:00
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
