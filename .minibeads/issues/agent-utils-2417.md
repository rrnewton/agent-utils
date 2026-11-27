---
title: 'headless-assistant-message-diagnostics: the headless wrapper records no per-message block kinds, so a reply written only in thinking needs the raw transcript to diagnose'
status: open
priority: 2
issue_type: feature
assignee: cl-bridgeretire
labels:
- agentctl
- chat
- headless
created_at: 2026-11-27T17:44:44-05:00
updated_at: 2026-11-27T17:44:44-05:00
---

# Description

Tracing #2386 (a headless coordinator wrote five reply blocks only in its thinking, so the bridge captured nothing) needed the session's raw Claude Code transcript. The wrapper's event log records `assistant.text` events but nothing about the assistant messages that held no text, so it could not show that 49 messages held only thinking and tool calls, with hundreds to thousands of output tokens each.

#2386 landed the prevention (a prompt sentence) and the detection while the turn runs (a mid-turn notice). This is its third design item, the diagnosis, split out because it needs its own design.

# Design

- The headless wrapper records one `assistant.message` event per assistant message. It holds the kinds of the message's content blocks (text, thinking, tool use), in order, and its output token count. It never holds thinking text, signatures or tool input.
- `chat sent N`, for a request with no reply, adds how many assistant messages since its prompt started held only thinking and tool calls, from those events.
- The event stays small and bounded: kinds as a short list or counts, the token count as a number. Older event logs without it read as before.
- Design note to coordb before any code. The event log is read by other tools, so report the new event type before gating.

# Acceptance Criteria

- `assistant.message` events carry block kinds and output tokens and never thinking text, signatures or tool input. A test with a fake harness stream that mixes thinking, text and tool use pins the event's exact fields.
- `chat sent N` reports the thinking-only message count for an unanswered request and nothing extra for an answered one, in a deterministic test.
- An event log written before the change still reads, and `chat sent` says the count is unknown rather than 0.
- The chat user guide documents the event and the `chat sent` line.

# Notes

- Split from #2386 (design item 3) on 2026-11-27. Related: #1974 (the headless transcript from the event log), #2331, #292.
