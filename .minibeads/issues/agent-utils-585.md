---
title: 'agent-watch-recovery: classify blocked harnesses and retry transient stops'
status: closed
priority: 2
issue_type: feature
created_at: 2026-10-27T16:01:30.595462304+00:00
updated_at: 2026-10-27T18:24:49.998359605+00:00
closed_at: 2026-10-27T18:24:49.998359551+00:00
---

# Description

[GPT-6 Codex] Delivered in source 59543339 and session-follow hotfix a76450de. The watcher classifies capacity, API rate limits, unsafe commands, safety refusals, quota and stalled goals. Only opted-in capacity/rate-limit conditions may retry, with persisted intent, bounded backoff and a three-attempt limit; safety, permissions and quota notify the coordinator. Exact-pin stale-registration retirement preserves archived queues and never stops the runtime. Session following validates supported launch metadata before rebinding queues, preserves process/terminal/generation pins and refuses competing identity claims.

The live deployment, final exact-head validation receipt and its limitations are recorded in ai_docs/transient/2026-10-27-agent-watch-recovery/VALIDATION.md. New regressions fail against the initial publication and pass with the hotfix. Original tests lacked a command line and therefore missed launch-spec validation; that coverage gap is fixed. Baseline failures remain tracked separately as #586 host-baseline-contract; polling reply order was fixed in #587 chat-reply-order.
