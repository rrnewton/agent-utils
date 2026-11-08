---
title: 'frozen-reclaimable-build-output: a frozen validation checkout with only ignored build output reads as not pristine'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-08T20:35:00.000000000+00:00
updated_at: 2026-11-08T20:35:00.000000000+00:00
---

# Description

`_retained_validation_git_state` (`py/wrkslots/cli.py`, reached through `_pristine_frozen_validation_state`) refuses to reclaim every frozen validation checkout in which a consumer ran a hermetic split build. Git collapses a fully ignored directory to a single `! ignored/` status line, so a tree whose only residue is pure cargo output (about 20 GB, 30,515 files, no symlinks, under `ignored/hermetic/split/target` in that consumer's layout) reads as "not pristine; preserve it". Each frozen checkout then keeps tens of GB that are regenerable build output, on a host whose `/data` is near its floor.

A consumer's agent wrote a fix (branch `musejanitor/frozen-split-target-residue`, head 9405416c on base a0eeb67d, in a worktree on devbig014). It hard-codes the consumer's path, which agent-utils must not do: consumer conventions live in configuration.

# Design

- Add a wrkslots configuration list of reclaimable build-output paths or globs (e.g. `reclaimable_ignored_paths`), relative to the checkout.
- When git reports a collapsed ignored directory, expand it, and treat the tree as pristine only if every ignored entry lies inside a configured reclaimable subtree. Match whole subtrees only. Any other ignored file (a stray log, a marker file, a sibling directory) still preserves the tree. Symlinks inside a reclaimable subtree preserve too, unless the design proves them safe.
- An empty or missing list keeps today's behaviour exactly.
- Mirror the change in the Rust edition, or record the parity gap for the Rust wrkslots owner.
- The consumer sets its own path in its config; no consumer path appears in agent-utils.

# Acceptance Criteria

- The consumer's residue test, adapted to the config: fails before the change and passes after.
- Variants still preserve the tree: a stray log beside the reclaimable subtree, a marker file inside the parent, an unrelated ignored file elsewhere.
- No configuration means no behaviour change (a test).
- Python/Rust parity is covered by a test, or the gap is recorded under Notes with its owner.
- `scripts/check_client_names.py` passes. The setting is documented in the wrkslots config reference.

# Notes

Requested by the lead (devbig014) on 2026-11-08 for review and landing on the consumer's behalf. Medium priority: it blocks reclaiming tens of GB per frozen checkout on devbig014.
