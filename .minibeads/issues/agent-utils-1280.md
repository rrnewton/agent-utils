---
title: 'wrkslots-kept-set-over-collection: private-history salvage skips published salvage refs and expired dangling commits'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-11T08:50:27+00:00
updated_at: 2026-11-11T08:50:27+00:00
---

# Description

[opus 5.5] While investigating #1278 wrkslots-salvage-bundle-object-store, one consumer slot's salvage commit for a nested module turned out to be an octopus with 3,871 parents: the slot HEAD plus 3,870 "kept:" commits. Reading the code, the private-history scan (`_submodule_private_history`, `_commit_tips`) collects too much in two ways.

- (a) **Published salvage isn't recognised.** `remote_cover` (`_remote_advertised_covering_uncached`) asks ls-remote only for `refs/heads/*` and `refs/tags/*`. A commit already published under the remote's `refs/salvage/*` (where wrkslots itself publishes salvage) and fetched back as `refs/remotes/origin/salvage/*` is never cleared, so every removal keeps it again.
- (b) **The all-objects sweep keeps ancient garbage.** Every commit object in the store (`cat-file --batch-all-objects`) is a root, so commits that git's own garbage rules would already prune are kept and bundled.

# Design

- (a) `remote_cover` also asks ls-remote for `refs/salvage/*`, and a commit those advertised refs reach counts as durable. Update the #1255/#1257 publication proof for the new advertised namespace.
- (b) Follow git's garbage rule. Read the repository's effective `gc.pruneExpire` (default 2 weeks). A commit with no ref and no reflog entry that is older than that is not kept: the receipt reports it as "dangling, past prune expiry". A dangling commit newer than that, such as a recently dropped stash or deleted branch, stays kept, as #1255 intends.
- Unchanged: stale remote-tracking tips kept by #1257, local heads and tags, and reflog-only or pseudoref-only tips.
- No lazy fetch. Fix the Python edition and Rust `remove_write.rs` together, keeping the Rust/Python differential exact.

# Acceptance Criteria

- In a fixture, commits reachable only from fetched-back salvage refs that the remote advertises are not kept.
- A dangling commit older than the effective prune expiry is reported, not kept. A newer one is still kept. Both use a fixed clock value passed to the code, never the wall clock.
- The receipt counts the kept commits by category: named refs, stale remote, dangling recent, and dangling past prune expiry.

# Notes

Lead ruling 2026-11-11: keep #1278 narrow and handle these two over-collection paths here.
