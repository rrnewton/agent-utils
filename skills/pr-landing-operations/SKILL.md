---
name: pr-landing-operations
description: "Coordinator doctrine for deciding and executing PR landings at scale: exact-head validation and review, drain ordering, coalescing, serialized merges, speculative lands, and ancestry proof. Use after pr-landing-planner emits a plan; this skill may drive mutations, while the planner never does and the consuming workspace AGENTS.md remains the authorization authority."
---

# PR landing operations

This skill owns **how a coordinator executes an authorized landing or large drain**. Keep the
boundaries explicit:

- [PR landing planner](../pr-landing-planner/SKILL.md) collects conflicts, evidence classes,
  mechanism overlaps, and assignments. It emits an advisory plan and never mutates a PR.
- This skill orders rebases, validation, review refresh, coalescing, merges, and post-merge proof.
- The consuming workspace's [`AGENTS.md`](https://github.com/rrnewton/dev-hermit/blob/main/AGENTS.md)
  defines who may authorize a landing, required review and checks, repository-specific merge rules,
  and task closure. This skill cannot weaken those rules.

Local validation is the main landing driver. GitHub CI is an independent supplement, not a
40-minute inner loop and not a reason to postpone evidence the local producer can generate now.

## Produce a qualifying local record

On the measured Meta agent sandbox, bare `./validate.sh` exits 3 in about nine seconds because
BpfJailer denies cgroup creation. That is admission refusal, not a test result. Use the consuming
workspace's registered producer, which launches a detached `systemd-run --user` unit and enters the
shared validation admission lock before invoking `validate.sh`:

```bash
./ci-hub/ci-hub validate-run --checkout <worktree> --agent <agent> \
  --target <exact-40-hex-head> --pr <number> -- full
```

This is not an unboxed bypass: systemd grants the cgroup scope and validation remains boxed. The
detached unit survives agent recycling. Record its unique unit and durable log. Stop the unit through
the repository stop command; killing the watching agent neither stops the run nor revokes its future
ledger write.

Read duration before interpreting the log. On the measured lane, about 9 seconds means admission
refusal, about 137 seconds often points to the build/lint stage, and roughly 400-700 seconds means
substantial execution. These ranges only classify where to look; they do not prove failure or full
coverage. The named-node receipt evidence remains the authority.

`result=pass`, a full check count, and an aggregate test count are not coverage evidence. A
qualifying Hermit green requires all of:

```text
result=pass AND profile=full AND exact tested SHA
AND coverage.planned_test_nodes > 0
AND coverage.zero_executed_nodes == []
AND coverage.absent_nodes == []
```

Never classify completeness from `executed_tests`, filtered-test totals, or a numeric band. Commit
`ee303899` produced eight legitimate partial passes at `executed_tests=427`; each correctly remained
outside full green because only 4 of 19 planned test nodes executed and 15 were absent. A two-check
`portable-strict-compat-only` row can likewise say `pass` and often lands first; it is not a full
green. Select by exact SHA, profile, and per-node coverage before ordering records by recency.

Classify a red only from `gates[]`: name the failing DAG node and cite that node's `exit_code`.
Aggregate counts do not establish failure or truncation. A run that executes nothing may be an
explicit `no_result`, but pre-classifier rows without named-node evidence remain unresolved until
reclassified; do not coerce them into failures.

Treat **soft green** only as a scheduling signal: no known product failure and enough evidence to
admit a PR to a chosen batch. It never authorizes landing by itself. **Hard green** is the qualifying
exact-head full receipt above (or the repository's authoritative equivalent), with exact-head review
resolved. State which one is being cited; never write only "green".

## Finalize identities before spending evidence

A receipt is SHA-keyed. Rebasing changes the SHA and destroys its authority. Rebase the entire chosen
wave to its final heads first, then validate each final head exactly once. Never interleave
rebase -> validate -> rebase -> validate: the second rebase discards the first receipt. That
circularity can turn a queue of truly green PRs into zero landable heads. A serial drain otherwise
pays N rebases and N exact-head validations. In the measured drain, 65 of 131 PRs were landable once
rebased; interleaving landing and validation would have invalidated the evidence for the remaining
heads after every main advance.

A review signature is SHA-bound too. Before trusting `passed-review-*`, dereference the corresponding
review evidence and require its reviewed SHA to equal the current PR head. A bare label, a review
without a recorded SHA, or an approval of an earlier head is not approval of the current code. Rebase,
amend, conflict resolution, and follow-up commits all invalidate prior review authority.

[Hermit PR #1200](https://github.com/rrnewton/hermit/pull/1200) is the concrete failure: its current
head `81a59a16` contains validated/reviewed `21ecb06b` plus two core-scheduler commits that the
`passed-review-codex` signer never examined. The label describes older code. Re-run required review at
the exact final head before landing.

## Choose serial or coalesced landing

For a large conflict-free backlog, a coordinator-authorized staging branch can reduce N repeated
integration updates and validations to one combined update and one full validation:

1. Fetch fresh and create staging from **current main**, never from a stale green anchor; the latter
   silently reverts everything landed after that anchor.
2. Merge every ready, conflict-free PR into staging. Skip conflicts instead of resolving them inside
   the batch; conflict resolution changes reviewed content and returns that PR to individual work.
3. Validate the exact combined staging head once. If main moves, update staging before validation;
   never claim the old receipt for the new head.
4. Land staging under the repository's normal authorization, lock, and protection rules.

Merging staging does not close constituent PRs. GitHub recognizes commit lineage, not content
equivalence. After a fresh fetch, close only a constituent whose original head satisfies:

```bash
git merge-base --is-ancestor <pr-head> refs/remotes/origin/main
```

An rc=0 is proof; rc=1 means the work is not proven landed and closing it can bury dropped content.
For a direct or rebase merge, fetch the named target and prove GitHub's `mergeCommit.oid` is its
ancestor. The API `MERGED` flag, PR head, successful `gh pr merge` exit, mergeability query, and clean
dry-run are not landing evidence.

## Serialize validation and merging

Never batch local validates concurrently. Contention has triggered `detcore_misc` livelock and
written false reds; recorded reds are not automatically retried, so an operator-created false red can
permanently condemn a healthy PR. Hold the validation lock and derive any future safe width from a
solo run's measured CPU and memory footprint, not host core count.

Serialize merges too. Concurrent stale-base rebase/admin merges orphaned already-merged work by
replaying incompatible views of main. Admin bypass is forbidden regardless. Hold one landing lock,
fetch fresh before each merge, and ancestry-verify afterward.

## Speculative lands and iteration

Speculative landing is limited to the cases authorized by the consuming `AGENTS.md`, such as
CI-irrelevant diffs or an explicitly urgent tooling fix. Its contract is binding: arm local
validation and GitHub CI on the landed commit immediately, in parallel, and act as soon as either
reports. A speculative land without armed exact-SHA verification is a process violation, not a
shortcut.

During implementation, reproduce and iterate on the single failing test locally. Never put full CI
in the edit/test inner loop. Use full local validation as post-hoc confirmation, and run local
validation and GitHub CI in parallel rather than serially.
