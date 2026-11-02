---
title: 'local-merge-queue: first-contact fixes (plan speed, regeneration failure kinds, retry-later), reconciled with a second lane'
status: in_progress
priority: 1
issue_type: feature
created_at: 2026-11-02T08:30:00+00:00
updated_at: 2026-11-02T13:30:00+00:00
---

# Description

[opus 5.5] Follow-up to #733 local-merge-queue, from a consumer's first shadow run (plan and dry run against 95 real pull requests, relayed by the coordinator 2026-11-02). With regeneration the queue found 39 of 95 landable in 20 batches, against 10 without, but:

1. Planning took about 25 minutes (thousands of serial `git` calls), so a plan was stale before it printed. Wanted: parallel probes bounded by a setting, probe answers cached by exact commits across plans, pairs skipped when they share no file outside the generated paths, a long-lived `git` batch process, and a before/after measurement.
2. A candidate whose tree cannot build the generator was ejected as a generator failure. Wanted: distinct kinds for a build failure (with the error tail), a generator failure, and a regeneration that touched other files.
3. Draft pull requests were silently skipped by `sync`; name them.
4. A validator refusal that means "try again later" must be retried, not read as a red validation.

While this lane was down, a second agent lane landed overlapping work on main (probe cache and parallel probes, refusal deferral, a regeneration-build kind, a daemon, notifications, persisted results, approval at head). This issue also covers reconciling the two (one implementation per feature, chosen by benchmark and tests) and fixing the correctness bugs an adversarial review found in the second lane's commits.
