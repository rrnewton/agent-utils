---
title: 'headless-tail-pretty: readable, colored agentctl tail with Markdown rendering'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
- headless
depends_on:
  agent-utils-565: parent-child
created_at: 2026-10-27T17:36:53.447460489+00:00
updated_at: 2026-10-27T17:36:53.447460489+00:00
---

# Description

[opus 5.5] Child of #565 headless-coordinator. The owner (2026-10-27) found the agentctl tail view of the headless trial too basic: no colors, and the agent's prose wraps at a fixed 100 columns as one long line. Pretty mode (default on a terminal; --plain or NO_COLOR for plain): colors per event kind (input received, delivered/started, assistant text, tool call and result, reply sent, interrupt, error), dimmed timestamps, a clear header per turn, assistant and reply text rendered as Markdown (paragraphs, lists, headings, code blocks, soft-wrapped to the terminal width keeping line breaks), and long tool output folded (first N lines, then the count of the rest).

# Acceptance Criteria

Snapshot tests of the renderer, colored and plain; width follows the terminal; tool output preview is bounded and masked; reply commands show as reply.sent with their text.
