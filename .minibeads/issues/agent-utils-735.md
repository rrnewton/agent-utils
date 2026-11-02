---
title: 'local-merge-queue: first-contact fixes (plan speed, regeneration failure kinds, retry-later), reconciled with a second lane'
status: closed
priority: 1
issue_type: feature
created_at: 2026-11-02T08:30:00+00:00
updated_at: 2026-11-02T18:52:14+00:00
closed_at: 2026-11-02T18:52:14+00:00
---

# Description

[opus 5.5] Follow-up to #733 local-merge-queue, from a consumer's first shadow run (plan and dry run against 95 real pull requests, relayed by the coordinator 2026-11-02). With regeneration the queue found 39 of 95 landable in 20 batches, against 10 without, but:

1. Planning took about 25 minutes (thousands of serial `git` calls), so a plan was stale before it printed. Wanted: parallel probes bounded by a setting, probe answers cached by exact commits across plans, pairs skipped when they share no file outside the generated paths, a long-lived `git` batch process, and a before/after measurement.
2. A candidate whose tree cannot build the generator was ejected as a generator failure. Wanted: distinct kinds for a build failure (with the error tail), a generator failure, and a regeneration that touched other files.
3. Draft pull requests were silently skipped by `sync`; name them.
4. A validator refusal that means "try again later" must be retried, not read as a red validation.

While this lane was down, a second agent lane landed overlapping work on main (probe cache and parallel probes, refusal deferral, a regeneration-build kind, a daemon, notifications, persisted results, approval at head). This issue also covers reconciling the two (one implementation per feature, chosen by benchmark and tests) and fixing the correctness bugs an adversarial review found in the second lane's commits.

# Notes

## Resolution

[opus 5.5] Delivered on main in d6bdb8e8, c3c9aa22 and 802fad3f.

1. Plan speed: the probes run on long-lived `git merge-tree --stdin` workers. Answers are cached across plans, keyed by exact commits and a configuration hash that includes the merge environment. Pairs with no shared file outside the generated paths are skipped. On the 95-change fixture, a cold plan went from 11 s / 265 CPU-s to 2.2 s / 6 CPU-s with the same plan. The first stage now decides every pair, so the "unknown pairs count as clean" note from the second lane no longer applies.
2. Regeneration runs per change, with kinds `regen-build-failed` (with the error tail), `regen-failed`, `regen-touched-other` and `regen-left-conflict`. A parent check stops the run without ejecting anyone when the base itself cannot regenerate.
3. `sync` names draft pull requests.
4. A refusal (exit 75, or a leading `COULD_NOT_RUN` line with no attributed failure) is retried, then deferred, and is never read as red.

Reconciliation: the second lane's interfaces were kept, with this lane's probe internals. Per-change regeneration replaced its hoisted refresh, which had three demonstrated bugs. Its refusal deferral and its daemon were kept, along with fixes for the 13 review findings, each with a regression test. Two points from the handover are pinned by tests. A concurrent landing survives the notification sweep (`sweep_keeps_a_concurrent_landing`). Approval at head fails closed when review data cannot be fetched (`a_failed_review_fetch_fails_closed`). 802fad3f also stops the serve preflight from refusing checkout-relative tool paths.

Remaining gaps move to #787 local-merge-queue.
