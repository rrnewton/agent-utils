---
title: 'merge-queue-push-rule-violation-permanent: GH013 repository rule violations are permanent for that candidate'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-06T12:04:31.632795051+00:00
updated_at: 2026-11-06T12:04:31.632795051+00:00
---

# Description

A consuming queue: GH013 linear-history push rejection retried as transient, ended run, cancelled flights, recurred next run; submit --exact accepted merge commit branch rules forbid.

# Design

Classifier permanent for GH013/Repository rule violations: one attempt, eject or hold quoting rule, other flights unaffected, run continues, never auto-retried. submit --exact/--preserve-merge and pre-validation check branch rules linear history, refuse merge commit early; unreadable rules warn. Per-repo exact_merge_allowed escape.

# Acceptance Criteria

Fake GH013 remote: one attempt, ejected/held with quoted rule, other flights unaffected, run continues, no retry later runs. submit --exact merge to linear-history branch refused before validation naming rule. Transient still retried.
