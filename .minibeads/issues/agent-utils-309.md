---
title: 'validate-nested-git-policy: make audit predict which nested Git repositories validation removal deletes'
status: open
priority: 3
issue_type: task
labels:
- wrkslots
created_at: 2026-10-14T18:09:01.475981477+00:00
updated_at: 2026-10-14T18:09:01.475981477+00:00
---

# Description

[opus 5.5] Split from #289 census-zombie-leader-threads, whose part (b) this is; #289 landed only part (a). On main the resumable audit cache census (_resume_audit_cache_root_unbounded and the bounded walk) refuses every nested .git with 'cache directory contains nested Git metadata', while completed validation removal (_judge_cache_nested_repositories, _judge_nested_git_repository, _assert_judged_git_metadata) deletes the nested repositories it judges disposable and records them as nested_git_evidence. Audit therefore reports BLOCKED for a validation slot whose removal would succeed, so it cannot predict what removal deletes. This fails closed; it is not a safety bug. Prior work: 47301bd5 on archived/salvage/agent-utils-wrkslots-final-review-20261010 (one named nested-Git policy: refuse, self-contained, any), written before main had its disposable-repository judgment; reconcile audit with that judgment rather than port the commit as it stands.

# Acceptance Criteria

For a validate record, audit accepts exactly the nested repositories that completed validation removal would judge disposable and refuses the rest with the existing message; one test runs audit and removal over the same tree and shows they agree; make validate passes.
