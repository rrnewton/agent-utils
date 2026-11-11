---
title: 'bang-answer-markdown: bang answers in proper Markdown'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-11T18:20:00+00:00
updated_at: 2026-11-11T18:20:00+00:00
---

# Description

[watermelon-full-preview] Owner reply 399 on coord7's `!who` answer: "it does give me one big line rather than nicely formatted markdown". The bang answers use single newline breaks, which Markdown renderers (vibe-talk, and Slack in some paths) fold into one paragraph.

# Design

- `!who`, `!who NAME`, and `!help` answers in proper Markdown: bold section headings (Pinned / Ephemeral), nested bullets following the managed-by tree instead of a "managed by X" suffix on every line, one bullet per agent as `name — harness/model — state`.
- Must render in both Slack (mrkdwn via the bridge's outbound formatting) and vibe-talk; standard Markdown bold and hyphen bullets do.

# Acceptance Criteria

- Tests on the rendered text for `!who`, `!who NAME`, and `!help`. No wall-clock budgets (owner rule 346).

# Notes

Owner reply 399, relayed by coordb. Prior art: #917 chat-bang-commands, #1335 bang-answer-visible.
