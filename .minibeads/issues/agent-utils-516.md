---
title: 'thread-card-max-lines: a thread card never shows more than K lines of its root'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
depends_on:
  agent-utils-484: related
  agent-utils-405: related
created_at: 2026-10-25T21:25:27.803783013+00:00
updated_at: 2026-10-25T21:25:27.803783013+00:00
---

# Description

[opus 5.5] The owner (2026-10-25): "I really don't like BIG thread cards. I asked not to summarize threads with only one reply (my message their message). But I HATE having big thread cards... So if the message-root is bigger than a certain size (let's say K lines, with default K set to 5 lines), let's summarize it even with a single-response and push the root message inward to be the first child. If summarization is broken/disabled then I still want to SPLIT this over-large message. Put only the first K lines in the thread card and append "..." to the end. Then display the remainder of the message as a virtual first child underneath with "..." prepended. So when I fully pack all threads I still get a fairly compact thread view without a giant thread card dominating it."

Wanted: a synced setting K (default 5 lines); a root longer than K lines is summarized from its first reply (overriding #484 summary-card-threshold's one-reply exemption for long roots only), the root pushed inward as the first child exactly as #405 root-under-summary does; without a summary (disabled, failing or pending) the card shows the first K lines and an ellipsis and a continuation row under it shows the rest, marked as the root's (same author and time, no actions of its own; reply, react and receipts act on the root). A summary arriving later switches layouts without moving the scroll. Packed stays compact; pack/unpack, tap-spine-to-card, read dimming and receipts keep working; deep links to the root land on the card.

# Acceptance Criteria

Page tests for a long root with one reply on the summarized and split paths, the K setting, and the switch when the summary lands; server unit tests for the threshold and the line proxy; a Chromium walk with screenshots at 393x852 and 1280x800; README and Help.
