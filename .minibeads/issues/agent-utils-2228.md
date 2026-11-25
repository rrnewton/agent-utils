---
title: 'agentctl-chat-reply-over-limit: an over-long chat reply is split or refused at store time, never retried forever in silence'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- chat
created_at: 2026-11-24T22:31:00-05:00
updated_at: 2026-11-24T22:31:00-05:00
---

# Description

A consumer bridge stored an agent's reply (`agentctl chat reply`; receipt outcome "stored", phase "pending"), and the send never reached Slack. `chat inspect` shows the reply stuck in phase "sending", with no provider message id. The bridge log shows why: every outbound attempt fails with `slack_error: --markdown must be at most 12000 bytes. (outcome=NotApplied, retryable=true)`, repeated every few minutes for over an hour. The provider error is permanent for that body, but it's classed as retryable, so the bridge retries it forever. `chat thread` keeps saying "send in progress or outcome unknown", and nothing tells the agent or operator that the reply can never be delivered.

# Design

- At `chat reply` (store) time, measure the rendered body against the provider's limit, after the outbound formatting the helper applies. If it's over, split it into ordered parts at paragraph or line boundaries, using the existing reply-splitting rules, each under the limit, and send them as consecutive replies in the same thread. Or refuse at store time with a clear error naming the limit and the size, so the agent can shorten it.
- Classify a provider "body too large" or "too long" error as NOT retryable (a terminal failure for that body). Mark the reply failed, with the error, in `chat inspect`, `chat sent` and `chat thread`, and surface it through the bridge's existing alarm path to the agent that wrote it.
- `chat held` (or a new `chat drop`) can drop a stuck "sending" reply without a restart. Today an operator has no supported way to cancel it.

# Acceptance Criteria

- Test (fake helper): a reply whose rendered body exceeds the limit is split into parts, each under the limit, sent in order in the same thread. Or, under refuse mode, it's refused at store time naming the limit.
- Test: a provider error stating the body is too large marks the reply failed (not retryable), shows the error in inspect, sent and thread, and raises the alarm once.
- Test: an operator can drop a stuck sending reply, and inspect then shows nothing pending for that request.

# Notes

Follow-up finding (same day): the outbound helper DID split the body (its log line: slack_format=markdown chunks=2, default max_chunk_bytes 11000), but the Slack CLI behind the helper refused chunk 0 as over its 12000-byte --markdown limit. The CLI counts its argument differently from the helper's raw byte count (escaping or non-ASCII encoding). So the fix must also size chunks the way the CLI counts, or re-split smaller on a size refusal, instead of retrying the same chunk.


Reported by a consumer team's coordinator on 2026-11-25 (UTC): a reply of more than 12000 bytes retried for over an hour with no surfaced failure. The thread and reply addressing were correct; the size alone caused it.
