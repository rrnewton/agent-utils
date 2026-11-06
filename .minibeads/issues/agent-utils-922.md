---
title: 'merge-queue-exact-merge-head: land a submitted merge commit with both parents intact'
status: closed
priority: 1
issue_type: feature
assignee: cl-sync
created_at: 2026-11-06T03:35:00.000000000+00:00
updated_at: 2026-11-06T04:30:00.000000000+00:00
closed_at: 2026-11-06T04:30:00.000000000+00:00
---

# Description

A consumer needs the merge queue to land an ancestry MERGE commit exactly as submitted, with both parents intact, so another history becomes an ancestor of its main branch. The queue cannot do this today:

- With `combine: "rebase"`, `Git::own_commits` (git.rs) lists `rev-list --no-merges`, and `cherry_pick_with` builds new single-parent commits. The merge commit is never a replay unit, so its second parent is dropped.
- The only exact-head path, `Git::fast_forwards` (git.rs around 805-811), refuses any merge in onto..head. `Runner::build` skips an `exact` item as not-fast-forward on that same check (run.rs around 2141), and `combine_with` uses it before `own_commits`.
- `Item.exact` has no submit flag. It is preserved from an old item (ops.rs around 274) or set when a peer queue change requires this change's exact head (ops.rs around 976-979). Its other effects are solo batching and no pin or recount rewrite.

The push landing driver itself is fine: it fast-forwards the base to `candidate.tip`, so a candidate that returns the merge head unchanged lands with both parents.

# Design

- `submit --exact` (alias `--preserve-merge`), persisted through `Submission` into `Item`. Relate it to the existing `exact` field and reuse it if it is the same concept.
- Add `Git::fast_forwards_exact_merge(onto, head)`: onto is an ancestor of head AND `head^1 == onto`; an arbitrary ancestor is not enough. Use it for exact items in place of the merge-rejecting `fast_forwards`, in `Runner::build` and in `combine_with` before `own_commits`.
- The candidate returns the submitted head UNCHANGED only when onto is the head's first parent. If the base has moved, refuse as not-fast-forward, and require the merge to be rebuilt and revalidated on the new base. NEVER replay an exact item.
- Exact items are always solo.
- Pin resolution and recount already skip exact items. Regeneration must be a no-op for them, otherwise the change is refused rather than rewritten.
- Rejected: flipping the queue-global `combine` to `merge`. That changes every landing, and it lands an untested queue-made merge commit above the head.

# Acceptance Criteria

- A merge commit directly on the base, submitted with `--exact`, lands with the identical SHA, and both parents are reachable from the base afterwards.
- The same head is refused as not-fast-forward after the base advances, and nothing is replayed.
- A merge head submitted without `--exact` keeps today's `--no-merges` replay.
- `--exact` is shown in `submit --help` and persists across a daemon restart.

# Notes

Reported 2026-11-05 10:30 PM ET by the lead coordinator as URGENT. It blocks a consumer's release import, whose release lane wrote the spec above from the code. In code and docs, refer to the consumer generically.

## Resolution

Landed as b179350a (gated as 0ce99c78: 27/27 merge-queue steps; rust.merge-queue.test 463 cases, 0 failed).
- `submit --exact` (alias `--preserve-merge`) sets the existing `Item.exact`; `Submission` gains a serde-defaulted `exact` field, and the Item JSON is unchanged.
- A merge whose first parent is the base lands unchanged, with both parents.
- It is refused as not-fast-forward after the base moves.
- Without `--exact`, a merge head is still replayed.
- All three acceptance tests are in rs/merge-queue/tests/cli_peers.rs.
- Follow-up: #924 merge-queue-conflict-scan-large-range.
