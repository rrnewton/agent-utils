---
title: 'thread-line-overlap: the line from a thread''s root never crosses the thread''s message boxes'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T22:07:13.981059130+00:00
updated_at: 2026-10-19T22:47:51.114157448+00:00
closed_at: 2026-10-19T22:47:51.114157308+00:00
---

# Description

[opus 5.5] The owner, 2026-10-19, of the desktop two-column layout with one thread open on the right: "With only one thread expanded and one colored arrow showing ... why does it CROSS over my (now-full-width) text boxes?" The thread's coloured line from its root on Main runs down the left edge of the right column on top of the left border of the agent's full-width message boxes, whose timestamps and text start right against it.

# Acceptance Criteria

In two columns with a thread open on the right, the line from its root to the card at the top runs in a gutter of its own, left of every message box of the thread and right of the divider, whole or clipped at Main's edge, at every window width two columns are drawn at. The agent's boxes still span the column; the owner's own boxes stay partial width. On a phone, where no such line is drawn, nothing changes. A Chromium walk measures the line against every box, failing before the change.
