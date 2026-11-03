---
title: 'chat-trace-format-tagging: trace posts get a separate label line, metadata and outbound escaping'
status: open
priority: 1
issue_type: feature
assignee: cl-start@devbig030
labels:
- agentctl
created_at: 2026-11-03T22:00:33.308647159+00:00
updated_at: 2026-11-03T22:00:33.308647159+00:00
---

# Description

[opus 5.5] OWNER request (2026-11-03 16:29 ET): #714 trace posts begin with "— after: <trigger>" glued to the text ("— after: owner message The ..."); read aloud it sounds like "m-dash after", and the line reads as garbled. Trace posts also lack a machine-readable marker, and bare file names and relative links in them are auto-linked or shown as raw markup.

# Design

- Move the trigger label to its own short leading line (for example an italic "after agent <name>") or into metadata only; never glue it to the text.
- Tag every trace post (root and replies) with message metadata, for example event_type "agentctl.trace" with payload {agent, trigger, trace_day}, through the existing post-metadata path (#465). Other providers get an equivalent marker where possible.
- Apply the #794-style outbound escaping the chat-reply path uses: bare names like preemptions.rs must not auto-link, and relative links must not show as raw link markup.

# Acceptance Criteria

- Tests: the label on its own line, the metadata on root and replies, and the escaping of bare names and relative links.
