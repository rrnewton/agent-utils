---
title: 'chat-reply-order: keep captured replies in durable prompt order across clock changes'
status: open
priority: 2
issue_type: bug
created_at: 2026-10-27T16:47:43.265144504+00:00
updated_at: 2026-10-27T17:17:54.344082907+00:00
---

# Description

[GPT-6 Codex] The exact publication gate for agent-watch-recovery exposed reversed delivery of two captured replies in test_new_prompt_echo_does_not_hide_an_uncaptured_earlier_request_reply[True]. Its old failed fixture was cleaned by the runner, so no exact wall-clock sample is claimed. Three forced timestamp/ordering cases reproduce reversal deterministically before the fix: whole-second versus fractional RFC3339 timestamps, timezone-offset strings, and equal receipt times with reversed directory enumeration. A fourth case covers a backwards receipt clock. Polling prompts already follow the durable queue ID (provider creation time plus request identity), but polling replies sorted receipt-time strings instead, allowing their order to disagree with prompt delivery. Use the same durable queue order for polling prompts and replies. Preserve both replies, their order and all input/lock assertions. An initial streaming-cache prototype failed two existing fairness tests, so that unrelated prototype and its test were removed; streaming admission/fairness behavior is preserved. Focused polling and unchanged streaming suites pass: 179 tests in 59.99 seconds; exact final publication validation remains required.
