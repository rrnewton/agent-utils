---
title: 'message-summary-toggle: collapsed-message summaries off by default, quiet on failure, and never wider than the screen'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-26T23:37:24.169381914+00:00
updated_at: 2026-10-26T23:37:24.169381914+00:00
---

# Description

[opus 5.5] The owner, 2026-10-26 16:29: "I'm actually really NOT liking the summaries of collapsed messages ... their little highlight color doesn't match the thread they're part of. It should. I also want a toggle to turn summarization off on messages. But I want to keep summarization on for thread cards." 16:31: unpacking a thread on a phone pushed its rows off the right edge of the screen (a row with a quote and a code block, and a SUMMARY FAILED label). 16:33: "The summary failed is very invasive. I'm fine with a subtle label but it makes the UI non functional. I can no longer see if it's archived or mark it as read."

Wanted:
1. Unpacked rows never wider than the screen; a code block scrolls inside its own box (393x852 and 412x915).
2. A failed summary is at most a small muted label; it never hides the row's read/archived state or its Done.
3. A synced setting in Settings > Messages, Summarize collapsed messages, default OFF; off, collapsed messages show their opening lines and no summary is asked for, and the server refuses to generate one. Thread-card summaries unaffected.
4. On, the summarised row's highlight is its thread's hue (--thread-hue), never a fixed accent.
