---
title: 'thread-card-markdown-and-cap: a thread card keeps its root''s formatting, splits only between lines, and its summary comes with the page'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
depends_on:
  agent-utils-489: related
  agent-utils-522: related
  agent-utils-405: related
created_at: 2026-10-26T12:09:48.748255314+00:00
updated_at: 2026-10-26T12:09:48.748255314+00:00
---

# Description

[opus 5.5] The owner (2026-10-26): "a long message with markdown formatting loses its formatting when it is promoted to a thread card ... it was pushed down into a child message and it still was missing its formatting. But then asynchronously updated several seconds later. Again, if our model is fast local state changes followed by syncs to the server, I don't understand why I would see these delays. And also if our policy is to always limit the size of the thread card. I didn't see that appear either." Then, after a restart: "it had not yet loaded the summary so it displayed the first message as split in the second half of that split displayed without markdown formatting. I wanted to split only at the line granularity and always retain markdown formatting of course. And also I don't understand why we would fail to load the summaries just as fast as the other messages."

Found: a raw card draws its root as plain lines (#489 cardLines over messageText), and #522's split cuts by the 36-character proxy mid-line and draws its continuation as plain text (continuationOf blanks content_html). A thread's summary reaches the page only through a read of the thread list, which on load waits for Main's read and otherwise runs at most once a minute; the server queues a thread for naming only when such a read sees it.

# Acceptance Criteria

Card and continuation drawn from the root's rendered body (lists, line breaks, code, links), split only between blocks, list items or lines, a numbered list continuing its numbering; the K-line cap holds for every raw card; a known summary is on the card from the read that brings the root (and from the saved copy on reload); while a summary is being written the card waits in the summary layout (root inward whole) and only the card's words change when it lands; page tests, server tests, a Chromium walk at 393x852 and 1280x800, and load-time numbers before/after.
