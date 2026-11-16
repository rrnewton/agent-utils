---
title: 'chat-permalink-qualified-ids: provider-qualified thread/message ids build no permalink, so every #658 link is empty'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-16T09:23:16.000000000+00:00
updated_at: 2026-11-16T09:23:16.000000000+00:00
---

# Description

[watermelon-full-preview] P1 regression in #658 chat-bridge-permalinks (landed 5cf046a2). `BridgeState::message_thread_permalink` and `message_permalink` in `rs/agentctl/src/chat_runtime.rs` pass `SavedMessage.thread_id` and `message_id` straight to `chat_links::slack_thread_permalink` / `slack_message_permalink`, which expect a bare Slack `ts` such as `1794772223.198289`. Real bridge records store provider-qualified ids: `thread_id` is `<conversation>/threads/<ts>` and `message_id` is `<conversation>/messages/<ts>` (for example `C0CC755QYSK/threads/1794772223.198289`). `permalink_segment` refuses those shapes, so every permalink is `None`. Live symptoms: `chat reply` warns `:thread:NNN has no local permalink; shown as plain text`, `chat status` shows `thread_permalink: null` for every retained request, prompt headers carry no thread link, and the vibe-talk reader resolves nothing. The existing tests passed because their fixtures use bare `ts` ids. Affects the Rust edition only: the Python edition has no permalink path (no `chat_links` equivalent, no short-refs writer), so there is nothing to mirror there.

# Design

One helper in `rs/agentctl/src/chat_links.rs` extracts the bare `ts` from either stored form: a bare `ts`, or `<conversation>/threads/<ts>` / `<conversation>/messages/<ts>`. It refuses when the embedded conversation does not match the message's `channel_id`, and refuses anything else. Use it everywhere a stored id becomes a permalink `ts`: `slack_thread_permalink` and `slack_message_permalink` (which transitively fixes prompt headers, `chat thread`, `chat status`, and reply-token rendering), and `short_refs_document`, whose `root_ts` / `root_message` / `message` / `thread_root_ts` fields must be bare `ts` values for vibe-talk's reader. An entry whose ids extract no `ts` is omitted from `short-refs.json` rather than written in a shape the reader cannot resolve. No stored record changes shape; no network call is added.

# Acceptance Criteria

- A top-level record with qualified ids (`thread_id` `<conversation>/threads/<ts>`, `message_id` `<conversation>/messages/<ts>`, same `ts`, not a thread reply) builds its thread permalink, and a thread-reply record (root `ts` in `thread_id`, its own `ts` in `message_id`) builds both its thread permalink and its message permalink carrying `?thread_ts=`.
- The helper refuses a qualified id whose embedded conversation differs from `channel_id`, and refuses any other shape; bare `ts` input keeps working unchanged.
- End to end: a reply citing `:thread:N` renders as a Slack link, the prompt header names the thread permalink, `chat status` shows a non-null `thread_permalink`, and `short-refs.json` carries bare `ts` values.
- Tests use the real record id shapes (qualified ids), covering a top-level message and a thread reply.

# Notes

Diagnosed from live bridge state (read-only): retained request records store the qualified forms above while `slack_workspace_url` is configured, yet no permalink is built. Extends #658 chat-bridge-permalinks.
