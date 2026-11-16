---
title: 'vibe-talk-cache-bypass-reads: place and first reads under the cache-first rule'
status: open
priority: 2
issue_type: task
labels:
- slack
- vibe-talk
created_at: 2026-11-16T02:00:00.000000000+00:00
updated_at: 2026-11-16T02:00:00.000000000+00:00
---

# Description

Inside vibe-talk, most page reads are cache-first, but the reads that ask the provider what the server does not know still go upstream first: placing a live message's thread, and any first read of a view the cache holds nothing of. An interactive Flat, lookup or range read can likewise fan out into per-thread upstream walks even when the cache holds a proven-whole copy. These are page-triggered upstream spends on the shared Slack bucket, against the server's own rule that a page's read rate never sets the upstream read rate.

# Design

Bring the remaining reads under the cache-first rule: answer from the cache's best copy and refresh upstream on the server's own schedule (the existing refresh claim and back-off), including thread placement and cache-empty first reads. An interactive Flat, lookup or range walk uses the proven-whole cache copy when one exists; an upstream walk becomes the cache filler's job, not the waiting read's.

# Acceptance Criteria

- Placing a live message whose thread the cache holds proven-whole makes zero upstream calls.
- A first read of an uncached view answers from the best available copy and schedules one server-side refresh, instead of waiting on an upstream walk per page read.
- An interactive Flat read over proven-whole threads makes no per-thread upstream calls; existing overlapping-read settle tests still pass.

# Notes

[watermelon-full-preview] Filed by the direct-Slack burn-down enumeration (owner request 105, 2026-11-15): the target model is client <> vibe-talk <> Slack, and this is one remaining path that reaches Slack without vibe-talk's cache or writer in between. Rates and the shared-budget accounting are in #1682 slack-rate-limit-budget.
