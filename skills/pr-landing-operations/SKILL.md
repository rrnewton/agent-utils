---
name: pr-landing-operations
description: Validate and execute an authorized PR landing plan with exact-revision evidence, serialized publication, and ancestry proof. Use after pr-landing-planner emits a plan and only within the consuming repository's rules.
---

# PR landing operations

The [PR landing planner](../pr-landing-planner/SKILL.md) emits an advisory plan and never mutates a
PR. This skill describes the execution discipline after the consuming repository has authorized the
work. Its repository rules remain authoritative for required checks, reviews, merge methods, branch
protection, and who may publish.

## Bind evidence to the final revision

Fetch the target branch before choosing a wave. Rebase or otherwise finalize each candidate first,
then validate and review that exact head. Any rebase, amend, conflict resolution, or follow-up commit
changes the identity and invalidates revision-bound evidence.

Classify validation from named checks and their recorded outcomes. An aggregate pass count or label
does not prove coverage unless repository policy says it does. Likewise, an approval of an older head
is not approval of the current head.

## Choose a landing shape

Use serial landing unless the repository explicitly permits a coalesced staging branch. For an
authorized coalesced landing:

1. Fetch and create staging from the current target branch.
2. Combine only ready, conflict-free heads; return conflicts for individual resolution and review.
3. Validate and review the exact combined staging head.
4. Publish through the repository's normal protection and authorization path.

If the target branch moves, refresh the candidate and repeat revision-bound checks. Do not claim an
older receipt for a new head.

## Serialize writers and prove publication

Hold the repository's publication lock, if any, and allow one writer at a time. Fetch immediately
before publishing. Never force-push, bypass branch protection, or use an administrative override
unless the repository rules explicitly require and authorize that exact action.

After publication, fetch again and prove ancestry against the named target:

```sh
git merge-base --is-ancestor <landed-revision> refs/remotes/origin/<target-branch>
```

Exit status zero proves ancestry; a successful merge command, UI state, or clean dry run does not.
Close or mark constituent work landed only when its revision, or the repository's documented
equivalent, is proven present on the target branch.

Use `pr-landing-planner --help` and `pr-landing-planner --userguide` for planner inputs and evidence
classes. Re-read the consuming repository's rules immediately before any mutation.
