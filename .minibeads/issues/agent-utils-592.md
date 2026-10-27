---
title: 'gh-paced-permission-403: a permission 403 must not start a host-wide cooldown'
status: closed
priority: 1
issue_type: bug
created_at: 2026-10-27T19:50:22.733284292+00:00
updated_at: 2026-10-27T20:14:35.401421978+00:00
closed_at: 2026-10-27T20:14:35.401421868+00:00
---

# Description

[opus 5.5] Reported by another harness: one hand-run `gh api GET repos/<owner>/<repo>/actions/permissions` returned a plain HTTP 403 because the endpoint needs admin rights. gh-paced treats every 403 as GitHub pushback and started a 15-minute host-wide cooldown (extended once). During a cooldown the git credential helper refuses, so every agent's git fetch and push on that host failed and dependent automation degraded.

Wanted:
1. Classify 403s. Rate-limit pushback (secondary or abuse wording, Retry-After, X-RateLimit-Remaining: 0) keeps the cooldown and honours Retry-After. A permission denial ("Resource not accessible by integration", "Must have admin rights to Repository", Not Found-style auth failures) with no rate-limit signal fails at once with a clear message and no cooldown. An unknown 403 gets a short local backoff for that one call, never a host-wide cooldown.
2. Refuse known admin-only API paths (actions/permissions, settings, hooks, rulesets, branch protection changes, collaborators) before they reach GitHub.
3. The git credential helper is blocked only by real rate-limit cooldowns.
4. Tests with fixture responses for each class; USER_GUIDE and QUICKSTART explain the classification.
