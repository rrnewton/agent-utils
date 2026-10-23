---
title: 'slack-text-fidelity: long Slack posts keep their quotes, line breaks and one row'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- slack
created_at: 2026-10-23T14:57:06.400790769+00:00
updated_at: 2026-10-23T14:57:06.400790769+00:00
---

# Description

[opus 5.5] Owner report (2026-10-23), from the Slack instance behind a CLI bridge:

1. slack-double-escape: a pasted '> quote' shows in Slack as a literal &gt; (the stored text holds &amp;gt;). vibe-talk escapes & < > once, as Slack mrkdwn requires; the bridge then posted that mrkdwn as the CLI's plain-text body (--text), which the CLI now escapes a second time and shows literally in a rich_text block. Fix in the bridge: post the mrkdwn body as mrkdwn (--slack-mrkdwn).
2. slack-newlines-lost: a plain-text post's stored text (its notification fallback) has every line break flattened to a space, while its rich_text block keeps them; the bridge read the flattened text. Fix in the bridge: read mrkdwn explicitly, and rebuild a plain-text post's line breaks from its rich_text block when the stored text differs from it only in whitespace.
3. slack-post-split: a bridge with no stated size was cut at 2000 characters. Default it to 4000 bytes (Slack's recommended message size, and the CLI's mrkdwn cap), counted as sent; the split already prefers a blank line, then a line break, and keeps line breaks within each part.
4. own-message-combine: a long Slack post is its first part in the channel and the rest as replies in that part's thread, a few seconds apart. The page never combined a thread root with anything, so in the thread the first part stood apart from its continuation. A root may now head a combined row with the same author's replies in its own thread inside the window; a root still never joins a row.

# Acceptance Criteria

A fake-bridge test posts a long multi-paragraph message with quotes: each part escaped exactly once, within 4000 bytes, cut at a paragraph, line breaks kept. A page test combines a root with its author's continuation replies in its thread and keeps other roots apart. A standalone test covers the bridge patch (post flag, read format, line-break rebuild).
