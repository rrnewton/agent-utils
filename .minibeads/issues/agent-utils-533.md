---
title: 'card-split-atomic-blocks: a split thread card never cuts inside a table, code block, quote or list item'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
depends_on:
  agent-utils-528: related
created_at: 2026-10-26T14:50:42.483175269+00:00
updated_at: 2026-10-26T15:24:00.599298439+00:00
closed_at: 2026-10-26T15:24:00.599298319+00:00
---

# Description

[opus 5.5] The owner (2026-10-26): a message with a 7x6 table "has a table rendering correctly but then it bounced to being a thread card and it turned into a giant card that split every cell of the table into a separate line... that is the kind of rendering glitch that should never happen. It should go straight to our split prefix+postfix view of the message. And THAT should split at line granularity and not break tables or code blocks..."

# Acceptance Criteria

The card split (#528 cardUnits) treats a table, a fenced code block, a blockquote and a list item as one unit; a first unit taller than K draws a compact placeholder line on the card and goes whole into the continuation, rendered as itself; the change from a plain row to a card draws the split view with no broken frame between; page tests with the real table shape and code blocks, and a Chromium screenshot.
