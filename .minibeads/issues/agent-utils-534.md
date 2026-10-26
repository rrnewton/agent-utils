---
title: 'markdown-tables-render: tables, headings and line spacing in Slack messages'
status: closed
priority: 1
issue_type: bug
created_at: 2026-10-26T14:56:01.000799953+00:00
updated_at: 2026-10-26T16:36:03.704135130+00:00
closed_at: 2026-10-26T16:36:03.704135+00:00
---

# Description

[opus 5.5] The owner, 2026-10-26: "looks like we are not rendering tables in our markdown either... Orc web seems to have better markdown rendering than us", then "the whitespace is bad... pretty squished together", and of a sample table: "looks fine on native Slack but rendered TOTALLY wrong in vibe talk" (the table silently missing).

Traced (read-only, devbig014 CLI and message cache):
1. An agent's Markdown table nested under a bullet: Slack's own markdown_text conversion stored the header row glued to the line before it and the next paragraph glued to the last row, in rich_text and in the text. Not ours; vibe-talk's cache holds it byte for byte (56 newlines both).
2. A top-level Markdown table becomes a native Slack table block. The CLI's text column leaves it out and ends with ", with interactive elements"; only rich_content has it. Ours (the private shim passes that text on).
3. Headings become header blocks, which rich_content omits and the text glues between neighbours (" TL;DR • first item").
4. Newlines in the middle of sentences in another agent's message came from a terminal soft-wrap at the pane width when its bridge captured the reply (lane mu-slackfmt), not from Slack or vibe-talk.

Delivered:
- Shim patch (private, applied by the coordinator): text rebuilt from rich_content; native tables become GFM tables, headings become bold paragraphs, unknown blocks show as [unsupported block: KIND], an unreadable table gets a visible note.
- vibe-talk: [render] single_newline = soft|hard, soft by default (owner's decision 2026-10-26: soft-wrap all single newlines, to revisit later); typed bullet lines become list items; file lines and an agent's title line keep their breaks; a glued table is repaired when the reading is unambiguous; more space between blocks and around headings.

Not done: code highlighting (lane cl-hl); an is-markdown detector (hook only: newline_rule).
