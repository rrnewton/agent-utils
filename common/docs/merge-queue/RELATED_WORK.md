# merge-queue: related work

This comparison is about **merge queues**: systems that decide which ready changes land on a
shared branch, in what order and in which batches, while keeping that branch green. It covers
public, open-source systems, with hosted products listed only for contrast. It was written in
2026-11, from public documentation and general knowledge. No code was copied from any of these
projects. merge-queue borrows algorithms, and the table says which ones.

## Open-source systems

| Project (license) | Relevant capability | Relationship to merge-queue |
| --- | --- | --- |
| [bors](https://github.com/rust-lang/bors) (MIT / Apache-2.0), and the original [homu](https://github.com/rust-lang/homu) and [bors](https://github.com/graydon/bors) it descends from | An approved change is merged onto a staging ref and tested there. The target branch is then fast-forwarded to exactly the commit that was tested. Testing is serial, with priority ordering and "rollups" that bundle small changes. | merge-queue adopts the central rule: the base moves only to a commit whose exact tree passed validation. It also adopts priority ordering. It differs by batching and speculating, rather than testing one change or one rollup at a time. |
| [bors-ng](https://github.com/bors-ng/bors-ng) (Apache-2.0) | Batches every approved change into one staging build. A red batch is split in halves, and each half is retried, until the culprit is isolated. | merge-queue adopts batch bisection by halves. It differs by rerunning only the tests that failed, never the whole build, and by planning batches from the exact conflict graph instead of taking everything that was approved. |
| [Zuul](https://opendev.org/zuul/zuul) (Apache-2.0) | Dependent pipelines. Each change is tested on top of every change ahead of it in the queue, speculatively and in parallel. A failure removes that change and restarts the ones behind it. The active window grows on success and shrinks on failure (AIMD). `Depends-On` expresses cross-change dependencies. | merge-queue adopts speculative stacking of batches (depth, default 2), discarding and rebuilding the candidates behind a red one, explicit dependencies, and (planned) an AIMD window. It differs by being one local process for one branch, with no Gerrit or event-stream integration. |
| [Marge-bot](https://github.com/smarkets/marge-bot) (BSD-3-Clause) | A bot for GitLab that rebases each assigned merge request onto the target, waits for CI, and merges. Its batch mode tests several requests together. | A similar rebase-then-validate loop, and the same preference for linear history (merge-queue's `rebase` combine mode). merge-queue builds candidates locally with Git plumbing, rather than pushing rebased branches and waiting for hosted CI. |
| [Prow Tide](https://github.com/kubernetes-sigs/prow) (Apache-2.0) | Merges pull requests that match label and status queries. It groups them into pools per branch, tests batches, and merges a batch when it passes. It is driven by a periodic sync loop rather than by events. | merge-queue shares the periodic, poll-driven shape (no webhooks) and the per-branch batching. Tide trusts CI contexts as a single green or red signal. merge-queue tracks the individual tests behind each result. |

## Hosted products (contrast only)

| Product | Note |
| --- | --- |
| GitHub merge queue | Built in for repositories hosted on GitHub. It builds temporary `merge_group` branches and runs the required checks on them, and it supports batching and group size. It needs hosted Actions or webhooks, and it treats checks as pass/fail contexts. |
| GitLab merge trains | Each merge request runs a pipeline on top of the ones ahead of it in the train. A failure drops that request and restarts the requests behind it. This is the same speculative idea as Zuul, on hosted pipelines. |
| Aviator, Mergify | Software-as-a-service queues with batching, parallel speculative queues and priority lanes. They receive repository events over the network. |

## How merge-queue differs

- **Local only.** It is one command-line tool that works on a Git clone. It needs no service, no
  inbound connection, no webhook and no app installation. It runs on hosts that accept no
  inbound traffic.
- **Validation is a set of tests.** Every run, and every submitter's claim, names its exact
  selection (`all`, or a set of `step` / `step::case` IDs) and the policy that chose it. A focused
  landing leaves a tracked obligation to widen to the complete suite.
- **Failed-test-only bisection.** Culprit search reruns only the failed tests, plus whatever they
  depend on. The systems above rerun the whole build or pipeline for each half.
- **Conflict-aware planning.** Batches are chosen from exact `git merge-tree` probes. When changes
  conflict, the plan prefers the set that lands the most changes this round, and tells each held
  change what would unblock it. The systems above learn about conflicts by attempting the merge.
- **No repair yet.** Like bors and bors-ng, merge-queue ejects a culprit with a report. It does not
  attempt to rewrite the change.

The comparison does not claim that batching, bisection or speculative testing are new. Its
purpose is to show which established ideas merge-queue builds on.
