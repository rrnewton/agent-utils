---
title: 'chat-trace-format-tagging: trace posts get a separate label line, metadata and outbound escaping'
status: closed
priority: 1
issue_type: feature
assignee: cl-start@devbig030
labels:
- agentctl
created_at: 2026-11-03T22:00:33.308647159+00:00
updated_at: 2026-11-04T01:00:50+00:00
closed_at: 2026-11-04T01:00:50+00:00
---

# Description

[opus 5.5] OWNER request (2026-11-03 16:29 ET): #714 trace posts begin with "— after: <trigger>" glued to the text ("— after: owner message The ..."); read aloud it sounds like "m-dash after", and the line reads as garbled. Trace posts also lack a machine-readable marker, and bare file names and relative links in them are auto-linked or shown as raw markup.

# Design

- Tag every trace post with Slack message metadata (owner, 2026-11-03 18:08 ET): event_type "agentctl_trace", payload {v:1, kind:"root"|"entry", agent, team, session, day (ET date), seq, trigger}, through the existing post-metadata path (#465). Other providers get an equivalent marker where possible.
- The root keeps the fixed title "<agent> main session trace, <date> ET". Entries drop the glued "— after:" prefix entirely: the trigger goes into metadata only.
- Apply the #794-style outbound escaping the chat-reply path uses: bare names like preemptions.rs must not auto-link, and relative links must not show as raw link markup.

# Acceptance Criteria

- Tests: the label on its own line, the metadata on root and replies, and the escaping of bare names and relative links.

# Notes

## Resolution

[opus 5.5] Delivered as the owner directed (18:08 ET), which replaces "the label on its own line" with no label at all.

- **No label.** Entries are the agent's text alone. The trigger (`owner message`, `reply CHAT_REPLY_007`, `agent lane-a`, ...) travels only in the metadata. The root keeps its fixed title "<agent> main session trace, <date> ET".
- **Metadata.** Every root and entry carries Slack message metadata: event_type `agentctl_trace`, payload `{v: 1, kind: "root"|"entry", agent, team, session, day (ET date), seq, trigger}`.
  - `team` is the agent-teams team whose declaration delivers to the bridge's registry; `session` is the agent record's native session id. Both are null when unknown.
  - `seq` counts the day's posts from 0 at the root and is kept in `trace-thread.json` across restarts.
  - Path: `chat_service::publish_tagged` sets it on the outbound transport, which adds an optional `metadata` key to the v1 send request. A request without metadata is unchanged. The Slack outbound helper validates the key (exactly event_type and an event_payload object, at most 4 KiB) and passes it to `slack.message send --metadata=file://…` for every post of the send. Warm spares are skipped, as for blocks.
  - An older helper refuses the key as `request_invalid` before posting anything. The driver then posts without metadata for the rest of its run, so the trace keeps flowing.
  - Other providers get no marker yet: only the Slack helper accepts the key, and the driver sends it only through a helper that does.
- **Escaping.** Outside code fences and inline code:
  - bare file names whose extension Slack's auto-linker takes for a domain (`preemptions.rs`, `rs/agentctl/src/cli.rs`, `notes.md`) are put in inline code;
  - Markdown links to relative paths become "label (`path`)".
  - Absolute links, anchors, addresses and code are untouched.
- **Tests:**
  - `bare_names_and_relative_links_are_escaped_outside_code`
  - `trace_metadata_marks_roots_and_entries`
  - `fenced_text_is_dropped_and_remainder_traced` and `batched_prompts_are_all_labelled` (no label in the text; trigger kept)
  - `trace_metadata_rides_on_the_wire_only_when_set` (root and reply requests)
  - the helper's `a_send_carries_its_metadata_to_every_post_and_bad_metadata_is_refused`
