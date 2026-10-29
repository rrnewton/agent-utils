---
title: 'chat-bridge-reply-permanent-failure: stop retrying permanently refused replies, fall back to top-level'
status: closed
priority: 1
issue_type: bug
created_at: 2026-10-29T12:31:44.235649304+00:00
updated_at: 2026-10-29T16:24:52.217486488+00:00
closed_at: 2026-10-29T16:24:52.217486377+00:00
---

# Description

[watermelon-full-preview] Owner request 2026-10-29 (chat-bridge-robustness batch), from a read-only look at a running bridge of another harness build.

A reply whose thread parent is a channel-level event (not a message) was retried on every pass: the outbound helper refuses it as operation_conflict (outcome NotApplied, retryable=false) and the bridge retried the identical operation for over a hundred attempts across many hours, so the reply never posted and the log filled with the same refusal.

Wanted: classify the outbound error; a failure the provider proves did not apply and cannot succeed on retry is never retried as-is. Record send attempts and the last failure on the reply, fall back to posting at the top level of the channel (or in the right thread) under a fresh operation ID, and when that also fails permanently, mark the reply failed with the reason recorded and let later replies proceed. chat health (see chat-bridge-repoint) reads send_attempts, last_send_failure_code, last_send_failure_at_millis and send_permanent_failure from the reply record.


Closed: landed on main as 525b06fe (gate rc=0, validate: OK; agentctl Rust tests 1745 passed standalone in 420 s). Unverified: no live bridge round-trip by this lane; coordinator deploys.
