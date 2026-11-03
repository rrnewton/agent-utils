---
title: 'merge-queue-change-features: per-change features and a proposal for one shared change classifier'
status: closed
priority: 1
issue_type: feature
assignee: cl-mq@devbig030
labels:
- merge-queue
created_at: 2026-11-03T16:12:59.057600878+00:00
updated_at: 2026-11-03T18:14:15.200096889+00:00
closed_at: 2026-11-03T18:14:15.200096769+00:00
---

# Description

[opus 5.5] Owner request (2026-11-03), part of the merge-queue instrumentation (#822): per-change features for the decision log and for future batching priors. The owner wants ONE change classifier shared with a consumer's test-selection bucketing, living in agent-utils (consumers may depend on agent-utils, never the reverse).

# Design

- Features per change: lines changed, files, touched components (bucketed), conflict-graph degree, trigger class, CI-path or not (#817's ci-path list), soft-green evidence.
- Before building the shared part: a short proposal note in the private harness repository's ai_docs covering the shared module (crate, API, data format) and how a consumer migrates its own component bucketing onto it. The shared part waits for the devbig014 coordinator's OK; queue-local features can land first.

# Acceptance Criteria

- Each decision-log record carries the listed features.
- The proposal note exists and is linked here before any shared-module code lands.
- Tests cover feature extraction on fixture changes (CI-only, multi-component, conflicting).
