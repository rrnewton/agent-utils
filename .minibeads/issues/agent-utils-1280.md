---
title: 'wrkslots-kept-set-over-collection: private-history salvage skips published salvage, expired dangling commits and expired fetched-only stale tips'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-11T08:50:27+00:00
updated_at: 2026-11-11T08:56:22+00:00
---

# Description

[opus 5.5] While investigating #1278 wrkslots-salvage-bundle-object-store, one consumer slot's salvage commit for a nested module turned out to be an octopus with 3,871 parents: the slot HEAD plus 3,870 "kept:" commits. Reading the code, the private-history scan (`_submodule_private_history`, `_commit_tips`) collects too much in two ways.

- (a) **Published salvage isn't recognised.** `remote_cover` (`_remote_advertised_covering_uncached`) asks ls-remote only for `refs/heads/*` and `refs/tags/*`. A commit already published under the remote's `refs/salvage/*` (where wrkslots itself publishes salvage) and fetched back as `refs/remotes/origin/salvage/*` is never cleared, so every removal keeps it again.
- (b) **The all-objects sweep keeps ancient garbage.** Every commit object in the store (`cat-file --batch-all-objects`) is a root, so commits that git's own garbage rules would already prune are kept and bundled.
- (c) **Stale remote-tracking tips dominate.** Real counts from the slot (read-only): its module `origin` is the parent checkout, not a hosted remote. It has 2,994 `refs/remotes/origin/*` refs, 136 tags, one local head and 39,687 commit objects. Only 4 refs fall under `refs/remotes/origin/salvage`, and the remote advertises no `refs/salvage/*`. So (a) explains at most 4 of the 3,870, and the bulk is stale tracking tips that #1257's stale-remote rule keeps.

# Design

- (a) `remote_cover` also asks ls-remote for `refs/salvage/*`, and a commit those advertised refs reach counts as durable. Update the #1255/#1257 publication proof for the new advertised namespace.
- (b) Follow git's garbage rule. Read the repository's effective `gc.pruneExpire` (default 2 weeks). A commit with no ref and no reflog entry that is older than that is not kept: the receipt reports it as "dangling, past prune expiry". A dangling commit newer than that, such as a recently dropped stash or deleted branch, stays kept, as #1255 intends.
- (c) Stale remote-tracking tips, judged against the remote:
  - **Covered:** a tip that any ref the remote advertises (ls-remote) still reaches is covered and never kept.
  - **Pushed from this slot:** if the tracking ref's reflog shows the slot pushed that tip ("update by push"), keep it whatever its age; the receipt reports it as "pushed from this slot, gone from remote".
  - **Fetched-only:** with no push entry, keep the tip if the tracking ref last changed within the effective `gc.pruneExpire` (use the reflog time, or the committer date when there is no reflog). If it is older, the receipt reports it as "stale remote, past prune expiry" and it is not kept.
  - Add the pushed and fetched-only split to the #1255/#1257 publication proof and to the report counts.
- Unchanged: tags stay kept, `refs/heads/*` is always the slot's own work, and reflog-only and pseudoref-only tips are kept as today.
- No lazy fetch. Fix the Python edition and Rust `remove_write.rs` together, keeping the Rust/Python differential exact.

# Acceptance Criteria

- In a fixture, commits reachable only from fetched-back salvage refs that the remote advertises are not kept.
- A dangling commit older than the effective prune expiry is reported, not kept. A newer one is still kept. Both use a fixed clock value passed to the code, never the wall clock.
- In a fixture, stale tips that another advertised ref still reaches are not kept. Pushed-from-slot stale tips are kept at any age, fetched-only stale tips are kept only within the prune expiry, and older ones are reported.
- The receipt counts the kept commits by category: named refs, stale pushed, stale fetched recent, stale fetched past prune expiry, dangling recent, and dangling past prune expiry.

# Notes

Lead rulings 2026-11-11: keep #1278 narrow and handle the over-collection paths (a), (b) and (c) here.
