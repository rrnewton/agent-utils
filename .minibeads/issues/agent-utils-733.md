---
title: 'local-merge-queue: consumer fit (generated files, exact commits, ready sources, request budget, cross-repo order)'
status: in_progress
priority: 1
issue_type: feature
created_at: 2026-11-02T04:00:00+00:00
updated_at: 2026-11-02T04:00:00+00:00
---

# Description

[opus 5.5] Follow-up to #732 local-merge-queue (closed), from a consumer's onboarding answers relayed by the coordinator (2026-11-01). The consumer lands by fast-forwarding its base to the exact validated commit, never carries a validation result through a rebase, validates with a full run of about 100 graph nodes (15-18 minutes, at most two at a time per host), and has 127 open pull requests. 70 of the 102 that need a rebase need it almost only because of two generated files.

Wanted, generic in this repository:

1. Generated-file regeneration: configured generated paths plus a regeneration command. When a candidate's only conflicts (or staleness) are in those paths, regenerate them on the candidate, commit, and validate as usual. Deterministic, reported.
2. Cross-repository ordering: a change may require a change in another configured repository. That change lands first, as an exact fast-forward, before the dependent candidate is validated, and the dependent never lands if it fails.
3. Ready-source adapters: a label and a ready-list file, plus a recommendation.
4. A request budget for the hosting service's API with batched calls, and fetches without credentials for public repositories.
5. Failed-tests-only reruns and flaky-pass semantics mapped onto a runner that can rerun only the failed nodes of an earlier run at the same commit.
6. An updated shadow kit whose plan and dry-run stages stay free of side effects.
