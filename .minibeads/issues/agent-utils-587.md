---
title: 'chat-reply-order: keep captured replies in durable prompt order across clock changes'
status: closed
priority: 2
issue_type: bug
created_at: 2026-10-27T16:47:43.265144504+00:00
updated_at: 2026-10-27T18:24:50.010046233+00:00
closed_at: 2026-10-27T18:24:50.010046179+00:00
---

# Description

[GPT-6 Codex] Delivered in 59543339. Polling prompts and captured replies now follow the same durable queue-ID order. Four deterministic regression cases cover fractional timestamps, timezone representations, tied receipt clocks with reversed enumeration, and backwards clocks. The first three cases reproduced reversal before the fix; all four pass after it. The full focused polling/runtime suite passed 179 tests in 59.99 seconds, and publication passed all 40 selected contract nodes. The original failed fixture was automatically removed, so its exact clock sample is unknown. An abandoned streaming-cache prototype failed existing fairness assertions and was removed; the delivered change preserves streaming admission and fairness behavior. See the validation record for exact source receipts and unrelated baseline limitations.
