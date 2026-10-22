---
title: 'summary-cards-full: always show an AI summary in full'
status: open
priority: 2
issue_type: task
created_at: 2026-10-22T14:37:18.460223346+00:00
updated_at: 2026-10-22T14:37:18.460223346+00:00
---

# Description

The owner, 2026-10-22: expanding/contracting SUMMARIES is a bug not a feature. If summaries are created with AI we roughly control their length and the point is they are already shortened. Eliding those with '...' messes that up. Always show the full summary if we actually have it. Only raw message text of unbounded length needs expanded/collapsed. Leave summary cards uncollapsible for now.

Build: wherever a card or row shows an AI summary instead of raw message text (thread cards in the Threads view and the right column, summarized message rows, the summary card above a root child), the summary is always shown in full: no line clamp, no ellipsis, no expand/collapse toggle on that text, and the collapsed-lines setting does not apply to it. Raw-text cards keep the collapse behaviour and the setting. The fold-the-thread-header-to-a-bar feature stays as is.
