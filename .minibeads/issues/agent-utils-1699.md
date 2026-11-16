---
title: 'slack-writes-single-writer: posts, reactions and mark-read through one writer'
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

Writes must reach Slack, but today they reach it around vibe-talk: the chat bridge's outbound helper posts, edits and deletes messages and adds acknowledgement and rejection reactions directly through a Slack client as the bridge app, with its own ledger and budget, while vibe-talk's own posts and its automatic mark-read form a second writer. Two writers mean two rate-limit back-offs, two receipt records, and no single account of what was sent. vibe-talk reads reactions but never adds one, so reactions have no server writer at all today.

# Design

Route bridge outbound posts, edits, deletes and reactions through vibe-talk's post lane, acting as the same app identity the bridge posts as today, so one writer owns the Slack budget, the operation-key and receipt record, and rate-limit back-off. Record vibe-talk's automatic mark-read in the same writer ledger. Delivery semantics are unchanged: an uncertain post is still reconciled before any repost, and a rejection reaction still requires the human-author check first.

# Acceptance Criteria

- A bridge reply, edit, delete or reaction made through vibe-talk appears in the one writer ledger with its receipt, and counts against the shared Slack budget.
- An uncertain post is reconciled through the cache-backed listing before any repost, and never posts twice.
- With vibe-talk unreachable, an outbound write refuses loudly instead of falling back to a direct Slack post.

# Notes

[watermelon-full-preview] Filed by the direct-Slack burn-down enumeration (owner request 105, 2026-11-15): the target model is client <> vibe-talk <> Slack, and this is one remaining path that reaches Slack without vibe-talk's cache or writer in between. Rates and the shared-budget accounting are in #1682 slack-rate-limit-budget.
