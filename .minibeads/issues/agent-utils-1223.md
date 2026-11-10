---
title: 'bridge-raw-headless: a bridge raw message loses its raw mark for headless agents'
status: open
priority: 2
issue_type: bug
assignee: mu-rsctl
created_at: 2026-11-10T16:40:00.000000000+00:00
updated_at: 2026-11-10T16:40:00.000000000+00:00
---

# Description

A raw bridge message (#1136 raw-message-bridge) submitted for a HEADLESS agent is queued without its `raw` mark, so the queued item is indistinguishable from an ordinary bridge input. The chain drops the flag silently: the bridge's `submit_raw` sets `DrainOptions::raw_prompt`, the delivery reaches `ManagedAgents::send_identified_with_runtime`, the headless branch calls `send_headless_for_chat`, and its `enqueue_for`/`enqueue_headless` (in `rs/agentctl/src/subagents/headless.rs`) build the queue document without ever reading `raw_prompt` — no `raw: true`, and the pane-side document builder (`agent::enqueued_item`, the one place a raw item is defined) is not used.

Today the text still reaches the harness unchanged, because the headless wrapper writes every item's text verbatim and never batches; the failure shows only against the #1136 contract: a raw turn is supposed to be its own marked item, never combined and never reordered, and that guarantee rests on the mark the headless path discards. Any future wrapper batching, or a reader that trusts `raw_prompt(document)`, gets the wrong answer for headless items.

# Design

- Thread the raw flag through the headless enqueue: `send_headless_for_chat` and `send_headless` pass `options.raw_prompt` (or an explicit argument) into `enqueue_for`/`enqueue_headless`, which mark the item with the same document shape `agent::enqueued_item` defines for a raw prompt (marked `raw`, text exactly as given, no deferred-time opening).
- Share one definition of a raw queue item between the pane drain and the headless enqueue, so the console items of #670 agentctl-console and bridge raw items for the same text build the same request, differing only in the authority they carry (owner identity from chat vs local peer credentials).
- Add a test that a raw bridge submission for a headless agent settles with `raw: true` on the processed item, and that the harness receives the text unchanged.

# Acceptance Criteria

- A bridge raw message to a headless agent produces a queue item carrying `raw: true` and the exact text; the processed item keeps the mark.
- The console and bridge raw items for the same text build the same harness request (one shared raw-item definition), covered by a test.
- Existing headless send and chat bridge suites stay green.

# Notes

Filed 2026-11-10 while scoping the #670 console follow-up: the console queues through the same `queue::enqueue_into` InboxV2 lane without the raw mark either, and the two raw paths should share one definition. Related: #1136 raw-message-bridge, #670 agentctl-console.
