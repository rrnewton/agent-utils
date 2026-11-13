---
title: 'wrkslots-finish-accounting-per-ref-cherry: finish accounting runs one git cherry per remote ref'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- finish
created_at: 2026-11-13T12:50:01.384292833+00:00
updated_at: 2026-11-13T12:50:01.384292833+00:00
---

# Description

[watermelon-full-preview] Finish commit accounting (#1364) in py/wrkslots/cli.py _classify_checkout_commits runs vcs.patch_equivalents(path, ref, head) = one git cherry <ref> HEAD for every ref under refs/remotes/<remote>/ (loop over for-each-ref). On a checkout with 4,127 remote refs (1,663 rescue/*) at ~0.7-5 s each, a finish ran 40+ minutes holding the project lock, so every wrkslots create timed out (900 s, twice). The same loop exists in rs/wrkslotsd finish_write.rs classify_commits.

# Design

Keep classification semantics exactly. Order: landed by ancestry or patch-id against the landed ref (one cherry, as now); then open by exact containment in a published ref (existing remote_refs_containing or one for-each-ref --contains pass); only still-unaccounted candidates need patch-equivalence against published refs. Patch equivalence in ONE pass: patch-ids of remaining candidates (git patch-id --stable over git show/diff-tree/log -p), then one bounded git rev-list/log --no-merges --remotes=<remote> --not <head> (bounded by merge base / landed ref) piped through patch-id once, and match; skip entirely when no candidate is left. Do accounting outside the project/configuration lock where the finish flow allows it (gather lock-free, revalidate under the lock, pattern of #1367/#1276). Check Rust parity.

# Acceptance Criteria

- A fixture with thousands of remote refs proves a constant number of git invocations (count calls, never time them) for finish accounting in the Python edition, with Rust parity.
- Equivalence cases are identical before and after: cherry-picked commit on a published branch -> open; on landed -> landed; none -> unaccounted.
- No wall-clock waits in tests; scratch project roots only.
- Existing finish/lifecycle suites still pass.

# Notes

Related: #1364 finish accounting, #1367 / #1276 lock-free gather patterns. Scratch project roots only; never run unlanded wrkslots against a shared registry.

## Resolution

Pending.
