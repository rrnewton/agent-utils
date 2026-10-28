---
title: 'sender-header-mandatory: every agentctl send and tick-hub notice names its sender'
status: open
priority: 2
issue_type: feature
assignee: cl-header
created_at: 2026-10-28T01:40:10.893739540+00:00
updated_at: 2026-10-28T01:40:10.893739540+00:00
---

# Description

[opus 5.5] Owner 2026-10-27 18:26: "I do like this concept of a mandatory header, though. It should be required from agentctl. And from Tick Hub." An unlabelled status line appeared as raw input in another harness's coordinator pane with no indication of who sent it; it did not come through agentctl's queue, so a lane most likely typed it with raw herdr pane send-text.

Wanted:
1. agentctl send (both editions; queued, direct pane, headless, agentcloud, --urgent) always opens the prompt with one parseable line naming the sender and route, e.g. [from cl-foo via agentctl · reply: agentctl send cl-foo]. The sender comes from the caller's registry identity, AGENTCTL_AGENT, the harness ancestry, or human/unknown with the host. No flag suppresses it; only the chat bridge's owner-message path (its own header format) goes without.
2. tick-hub: every delivered notice carries [tick-hub: <name> · <schedule>].
3. agent-teams envelopes: make their header consistent with the new format.
4. herdr glue we control refuses to type into a pane without the header; AGENTS.md and briefs say agents message each other with agentctl send, never raw pane typing.
Tests in both editions (differential), docs (userguide, skills).
