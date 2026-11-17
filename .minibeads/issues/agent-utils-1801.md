---
title: 'agentctl-trace-posts-expand-short-refs: trace posts expand short references like replies'
status: open
priority: 1
issue_type: task
labels:
- agentctl
created_at: 2026-11-17T22:05:00.000000000+00:00
updated_at: 2026-11-17T22:05:00.000000000+00:00
---

# Description

[watermelon-full-preview] Follow-up of #1796 vibe-talk-bare-short-refs-render
(owner thread 141; coordb agreed 2026-11-17, separate change). Replies expand
`:thread:NNN`/`:email:NNN`/`:issue:NNN` into links at send time (#658/#1709),
but trace posts (#714 chat-transcript-primary, #832 chat-trace-format-tagging)
never pass through that expansion: the trace collector only escapes the text
(`chat_trace::escape_for_chat`), so a citation in a trace post reaches the
chat exactly as written and shows literally (no emoji, no link) there.
vibe-talk draws such references as short links since #1796; the chat itself
should too.

# Design

At the trace send path, and only there: run the same expansion the reply
path uses (`chat_links::render_reply_links`, or
`render_reply_links_markdown` on the converted format, exactly as
`chat_runtime::render_outbound_links_as` chooses for replies, #1709),
through one `BridgeState::render_trace_links` the driver calls with the
bridge state read once per pass. The stored trace text stays exactly as
the agent wrote it; links are rendered on the wire only, as for replies
(#658), so retries render the same. Link warnings and bare-number guard
notices from the expansion go to the service log, never into the trace
thread, and guard notices are NOT recorded in `bare-ref-warnings.json`
(that count reads replies; trace narration would flood it). The #832
escaping still applies (file names and relative links protected);
expansion runs on the escaped text it posts, and code spans/fences stay
unexpanded, as `render_reply_links` already guarantees. Because expansion
can carry a post past the collector's 3,500-byte split, the wire text is
re-split by the same splitter (`chat_trace::split_wire`) and each part
posts under its own derived request id, so retries re-derive the same
parts and the publish ledger answers for the ones that landed. No change
to collection, rate limiting, metadata, or the reply path.

# Acceptance Criteria

- Trace-post unit tests (red before, green after): a post citing
  `:thread:060`, `:email:1846`, `:issue:658` is sent with the same link
  markup a reply with that body is sent with (Slack form, and Markdown
  form on the converted path); unresolvable references stay plain;
  references inside code spans/fences stay as written; the stored post
  text is byte-identical to the agent's text.
- A post just under 3,500 bytes with 10+ resolvable references yields
  wire parts each at most 3,500 bytes with every link intact; a trace
  render with guard notices leaves `bare-ref-warnings.json` unchanged;
  escaped trace text (`escape_for_chat`: a file name, a relative link, a
  `:thread:001`) expands its reference and keeps its escapes
  byte-identical.
- Existing trace and reply tests stay green (full agentctl Rust test
  module for the touched files).

# Notes

[watermelon-full-preview] mu-refrender. Agreed with coordb as the
separate change after #1796 (587f4321) landed; the diff goes to coordb
before the gate.
